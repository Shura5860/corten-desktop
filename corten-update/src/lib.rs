use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_REPO: &str = "Shura5860/corten-desktop";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_notes: String,
    pub tarball_url: String,
    pub sha256_url: Option<String>,
    pub published_at: String,
    pub asset_size_bytes: Option<u64>,
}

#[derive(serde::Deserialize)]
struct GitHubReleaseResponse {
    tag_name: String,
    body: Option<String>,
    published_at: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

pub struct CortenUpdater;

impl CortenUpdater {
    /// Queries GitHub Releases API for the latest release metadata
    pub async fn check_for_updates() -> Result<UpdateInfo, String> {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            GITHUB_REPO
        );

        let output = tokio::process::Command::new("curl")
            .args([
                "-s",
                "-H",
                "User-Agent: CortenDesktop-OTA-Updater",
                "-H",
                "Accept: application/vnd.github.v3+json",
                "--max-time",
                "6",
                &url,
            ])
            .output()
            .await
            .map_err(|e| format!("curl çalıştırılamadı: {}", e))?;

        if !output.status.success() {
            return Err("GitHub API sunucusuna bağlanılamadı".into());
        }

        let body = String::from_utf8_lossy(&output.stdout);
        if body.contains("\"message\": \"Not Found\"") {
            return Err("Henüz bir GitHub Release yayınlanmamış".into());
        }

        let release: GitHubReleaseResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Release JSON ayrıştırma hatası: {}", e))?;

        let latest_tag = release.tag_name.trim_start_matches('v').to_string();
        let current_ver = CURRENT_VERSION.trim_start_matches('v');

        let has_update = Self::is_newer_version(&latest_tag, current_ver);

        let mut tarball_url = String::new();
        let mut sha256_url = None;
        let mut asset_size = None;

        for asset in release.assets {
            if asset.name.ends_with(".tar.gz") {
                tarball_url = asset.browser_download_url;
                asset_size = asset.size;
            } else if asset.name.ends_with(".sha256") || asset.name.ends_with(".sha256sum") {
                sha256_url = Some(asset.browser_download_url);
            }
        }

        if tarball_url.is_empty() {
            // Fallback to release tag tarball
            tarball_url = format!(
                "https://github.com/{}/archive/refs/tags/{}.tar.gz",
                GITHUB_REPO, release.tag_name
            );
        }

        Ok(UpdateInfo {
            current_version: format!("v{}", current_ver),
            latest_version: format!("v{}", latest_tag),
            has_update,
            release_notes: release
                .body
                .unwrap_or_else(|| "Düzenli bakım ve performans iyileştirmeleri.".into()),
            tarball_url,
            sha256_url,
            published_at: release.published_at.unwrap_or_else(|| "Bilinmiyor".into()),
            asset_size_bytes: asset_size,
        })
    }

    /// Compares two semantic version strings (e.g. "0.2.0" vs "0.1.0")
    pub fn is_newer_version(latest: &str, current: &str) -> bool {
        let parse_parts = |s: &str| -> Vec<u32> {
            s.split('.')
                .filter_map(|p| p.split('-').next().and_then(|v| v.parse().ok()))
                .collect()
        };

        let l_parts = parse_parts(latest);
        let c_parts = parse_parts(current);

        for (l, c) in l_parts.iter().zip(c_parts.iter()) {
            if l > c {
                return true;
            } else if l < c {
                return false;
            }
        }

        l_parts.len() > c_parts.len()
    }

    /// Downloads and applies the release tarball atomically
    pub async fn apply_update<F>(info: &UpdateInfo, progress: F) -> Result<(), String>
    where
        F: Fn(&str),
    {
        progress(&format!("Güncelleme hazırlanıyor: {}", info.latest_version));

        // Use a user-private 0o700 staging directory (never world-writable /tmp) to prevent TOCTOU root escalation
        let base_cache = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok()
            .or_else(|| directories::BaseDirs::new().map(|b| b.cache_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let tmp_dir = base_cache.join(format!("corten_ota_update_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).map_err(|e| format!("Geçici dizin oluşturulamadı: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp_dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("Geçici dizin izinleri ayarlanamadı: {}", e))?;
        }

        let tarball_path = tmp_dir.join("update.tar.gz");

        // 1. Download Tarball
        progress(&format!("İndiriliyor: {}", info.tarball_url));
        let dl_status = tokio::process::Command::new("curl")
            .args([
                "-L",
                "-f",
                "--proto",
                "=https",
                "--max-time",
                "120",
                &info.tarball_url,
                "-o",
                tarball_path.to_str().ok_or("Geçersiz geçici dosya yolu")?,
            ])
            .status()
            .await
            .map_err(|e| format!("İndirme başlatılamadı: {}", e))?;

        if !dl_status.success() {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err("Güncelleme paketi indirilemedi (ağ hatası)".into());
        }

        // Verify downloaded archive is non-empty and matches expected size if provided
        let dl_size = fs::metadata(&tarball_path).map(|m| m.len()).unwrap_or(0);
        if dl_size == 0
            || info
                .asset_size_bytes
                .is_some_and(|expected| expected > 0 && dl_size != expected)
        {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err(format!(
                "İndirilen arşiv boyutu doğrulanamadı (alınan: {} bayt, beklenen: {:?}).",
                dl_size, info.asset_size_bytes
            ));
        }

        // 2. Extract Tarball
        progress("Güncelleme paketi açılıyor ve doğrulanıyor...");
        let extract_status = tokio::process::Command::new("tar")
            .args([
                "-xzf",
                tarball_path.to_str().ok_or("Geçersiz arşiv yolu")?,
                "--no-same-owner",
                "-C",
                tmp_dir.to_str().ok_or("Geçersiz geçici dizin yolu")?,
            ])
            .status()
            .await
            .map_err(|e| format!("Arşiv açılamadı: {}", e))?;

        if !extract_status.success() {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err("Arşiv açma işlemi başarısız oldu".into());
        }

        // 3. Locate extracted binaries folder
        let mut bin_dir = None;
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let sub_bin = path.join("bin");
                    if sub_bin.exists() {
                        bin_dir = Some(sub_bin);
                        break;
                    }
                }
            }
        }

        let bin_src = bin_dir.unwrap_or_else(|| tmp_dir.clone());

        // 4. Detect Install Location & perform an ACTUAL write probe (metadata.readonly() is always false for /usr/local/bin!)
        let target_prefix = Self::detect_install_prefix();
        progress(&format!("İkililer kuruluyor -> {:?}", target_prefix));

        let probe_path = target_prefix.join(format!(".corten_ota_probe_{}", std::process::id()));
        let is_writable = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)
        {
            Ok(_) => {
                let _ = fs::remove_file(&probe_path);
                true
            }
            Err(_) => false,
        };

        let binaries = [
            "corten-compositor",
            "corten-session",
            "corten-welcome",
            "corten-panel",
            "corten-launcher",
            "corten-notify",
            "corten-settings",
            "corten-lock",
            "corten-greeter",
            "corten-widgets",
            "corten-files",
            "corten-monitor",
            "corten-snipper",
            "corten-wallpaper",
            "corten-polkit",
            "corten-portal",
            "cortenctl",
            "corten-ai",
            "corten-forge",
            "corten-update",
        ];

        if is_writable {
            // Unprivileged user-writable location (e.g. ~/.local/bin)
            for bin_name in &binaries {
                let src = bin_src.join(bin_name);
                if src.exists() {
                    let dest = target_prefix.join(bin_name);
                    let _ = fs::remove_file(&dest);
                    fs::copy(&src, &dest)
                        .map_err(|e| format!("{} kopyalanamadı: {}", bin_name, e))?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
                    }
                }
            }
        } else {
            // System-wide install requires privilege escalation inside our 0o700 private staging dir
            progress("Sistem dizinine yazmak için yetki doğrulanıyor...");
            let install_script = tmp_dir.join("apply_sys.sh");
            let mut script_content = String::from("#!/bin/sh\nset -e\n");
            for bin_name in &binaries {
                let src = bin_src.join(bin_name);
                if src.exists() {
                    let dest = target_prefix.join(bin_name);
                    script_content.push_str(&format!(
                        "install -m 755 '{}' '{}'\n",
                        src.display(),
                        dest.display()
                    ));
                }
            }

            fs::write(&install_script, script_content)
                .map_err(|e| format!("Kurulum betiği yazılamadı: {}", e))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&install_script, fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("Betik izinleri ayarlanamadı: {}", e))?;
            }

            let pkexec_status = tokio::process::Command::new("pkexec")
                .arg(install_script.to_str().ok_or("Geçersiz betik yolu")?)
                .status()
                .await
                .map_err(|e| format!("pkexec çalıştırılamadı: {}", e))?;

            if !pkexec_status.success() {
                let _ = fs::remove_dir_all(&tmp_dir);
                return Err("Yönetici yetkisi verilmedi, güncelleme iptal edildi".into());
            }
        }

        // 5. Live Reloading / Notifications
        progress("Masaüstü servisleri canlı yenileniyor...");
        Self::hot_reload_services();

        progress(&format!(
            "Corten Desktop başarıyla {} sürümüne güncellendi!",
            info.latest_version
        ));

        // Desktop Notification
        let _ = Command::new("notify-send")
            .args([
                "-a",
                "Corten OTA",
                "-i",
                "system-software-update",
                "Corten Desktop Güncellendi",
                &format!(
                    "Sisteminiz başarıyla {} sürümüne güncellendi.",
                    info.latest_version
                ),
            ])
            .spawn();

        let _ = fs::remove_dir_all(&tmp_dir);
        Ok(())
    }

    fn detect_install_prefix() -> PathBuf {
        if let Ok(output) = Command::new("which").arg("corten-compositor").output()
            && output.status.success()
        {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(parent) = Path::new(&path_str).parent() {
                return parent.to_path_buf();
            }
        }

        // Fallbacks
        if let Some(home) = directories::BaseDirs::new() {
            let user_bin = home.home_dir().join(".local/bin");
            if user_bin.exists() {
                return user_bin;
            }
        }

        PathBuf::from("/usr/local/bin")
    }

    fn hot_reload_services() {
        info!("Hot-reloading Corten Desktop UI components...");
        // Signal components that actually handle SIGUSR1 (never send unhandled SIGHUP to corten-panel!)
        let _ = Command::new("pkill")
            .args(["-USR1", "-x", "corten-widgets"])
            .spawn();
        let _ = Command::new("pkill")
            .args(["-USR1", "-x", "corten-wallpaper"])
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version_semver() {
        assert!(CortenUpdater::is_newer_version("0.2.0", "0.1.0"));
        assert!(CortenUpdater::is_newer_version("0.1.1", "0.1.0"));
        assert!(CortenUpdater::is_newer_version("1.0.0", "0.9.9"));
        assert!(CortenUpdater::is_newer_version("0.1.0.1", "0.1.0"));

        assert!(!CortenUpdater::is_newer_version("0.1.0", "0.1.0"));
        assert!(!CortenUpdater::is_newer_version("0.0.9", "0.1.0"));
        assert!(!CortenUpdater::is_newer_version("0.1.0", "0.2.0"));
    }
}
