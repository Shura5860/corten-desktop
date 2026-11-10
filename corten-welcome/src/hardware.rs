//! Hardware and system diagnostics for Corten Welcome

use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub cpu: String,
    pub ram: String,
    pub gpu: String,
    pub compositor: String,
    pub kernel: String,
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self::detect()
    }
}

impl SystemInfo {
    pub fn detect() -> Self {
        let cpu = detect_cpu();
        let ram = detect_ram();
        let gpu = detect_gpu();
        let kernel = detect_kernel();
        let compositor = "Corten Compositor (Smithay Rust Wayland)".to_string();

        Self {
            cpu,
            ram,
            gpu,
            compositor,
            kernel,
        }
    }
}

fn detect_cpu() -> String {
    if let Ok(contents) = fs::read_to_string("/proc/cpuinfo") {
        for line in contents.lines() {
            if line.starts_with("model name") {
                if let Some((_, model)) = line.split_once(':') {
                    return model.trim().to_string();
                }
            }
        }
    }
    "x86_64 Modern İşlemci".to_string()
}

fn detect_ram() -> String {
    if let Ok(contents) = fs::read_to_string("/proc/meminfo") {
        for line in contents.lines() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        let gb = kb as f64 / (1024.0 * 1024.0);
                        return format!("{:.1} GB RAM", gb);
                    }
                }
            }
        }
    }
    "Sistem Belleği".to_string()
}

fn detect_gpu() -> String {
    let drm_path = Path::new("/sys/class/drm");
    if drm_path.exists() {
        if let Ok(entries) = fs::read_dir(drm_path) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with("card") && !file_name.contains('-') {
                    let uevent_path = entry.path().join("device/uevent");
                    if let Ok(uevent) = fs::read_to_string(&uevent_path) {
                        for line in uevent.lines() {
                            if line.starts_with("DRIVER=") {
                                let driver = line.trim_start_matches("DRIVER=").trim().to_lowercase();
                                match driver.as_str() {
                                    "nvidia" => return "NVIDIA GPU (Vulkan/DRM)".to_string(),
                                    "amdgpu" | "radeon" => return "AMD Radeon GPU (Mesa/Vulkan)".to_string(),
                                    "i915" | "xe" => return "Intel Iris/Arc Graphics (Mesa)".to_string(),
                                    "nouveau" => return "Nouveau Open Source GPU".to_string(),
                                    other if !other.is_empty() => return format!("{} Graphics Driver", driver),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    "Wayland DRM Hızlandırmalı GPU".to_string()
}

fn detect_kernel() -> String {
    if let Ok(kernel) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        let trimmed = kernel.trim();
        if !trimmed.is_empty() {
            return format!("Linux {}", trimmed);
        }
    }
    "Linux".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_info_detection() {
        let info = SystemInfo::detect();
        assert!(!info.cpu.is_empty());
        assert!(!info.ram.is_empty());
        assert!(!info.gpu.is_empty());
        assert!(!info.kernel.is_empty());
        assert!(info.compositor.contains("Corten"));
    }

    #[test]
    fn test_detect_cpu_fallback_or_proc() {
        let cpu = detect_cpu();
        assert!(!cpu.is_empty());
    }

    #[test]
    fn test_detect_ram_fallback_or_proc() {
        let ram = detect_ram();
        assert!(!ram.is_empty());
    }

    #[test]
    fn test_detect_gpu_fallback_or_proc() {
        let gpu = detect_gpu();
        assert!(!gpu.is_empty());
    }

    #[test]
    fn test_detect_kernel_fallback_or_proc() {
        let kernel = detect_kernel();
        assert!(kernel.starts_with("Linux"));
    }
}

