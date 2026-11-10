use std::fs;

#[derive(Debug, Clone, Default)]
pub struct SysMetrics {
    pub cpu_usage: u8,
    pub ram_pct: u8,
}

pub struct SysMonitor {
    prev_total: u64,
    prev_idle: u64,
}

impl SysMonitor {
    pub fn new() -> Self {
        Self {
            prev_total: 0,
            prev_idle: 0,
        }
    }

    pub fn sample(&mut self) -> SysMetrics {
        let (cpu, new_total, new_idle) = self.calculate_cpu();
        self.prev_total = new_total;
        self.prev_idle = new_idle;

        let ram_pct = Self::sample_ram();

        SysMetrics {
            cpu_usage: cpu,
            ram_pct,
        }
    }

    fn calculate_cpu(&self) -> (u8, u64, u64) {
        if let Ok(content) = fs::read_to_string("/proc/stat") {
            if let Some(first_line) = content.lines().next() {
                let parts: Vec<&str> = first_line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let user: u64 = parts[1].parse().unwrap_or(0);
                    let nice: u64 = parts[2].parse().unwrap_or(0);
                    let system: u64 = parts[3].parse().unwrap_or(0);
                    let idle: u64 = parts[4].parse().unwrap_or(0);
                    let iowait: u64 = parts.get(5).and_then(|v| v.parse().ok()).unwrap_or(0);

                    let total = user + nice + system + idle + iowait;

                    if self.prev_total > 0 && total > self.prev_total {
                        let total_diff = total - self.prev_total;
                        let idle_diff = idle.saturating_sub(self.prev_idle);
                        let usage = ((total_diff - idle_diff) * 100) / total_diff;
                        return (usage.min(100) as u8, total, idle);
                    }
                    return (0, total, idle);
                }
            }
        }
        (0, 0, 0)
    }

    fn sample_ram() -> u8 {
        if let Ok(content) = fs::read_to_string("/proc/meminfo") {
            let mut total_kb: u64 = 0;
            let mut available_kb: u64 = 0;

            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    total_kb = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    available_kb = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                }
            }

            if total_kb > 0 {
                let used_kb = total_kb.saturating_sub(available_kb);
                let pct = (used_kb * 100) / total_kb;
                return pct.min(100) as u8;
            }
        }
        0
    }
}
