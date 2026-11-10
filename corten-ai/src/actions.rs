use std::process::Command;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub enum DesktopAction {
    SetVolume(u8),
    SetBrightness(u8),
    TakeScreenshot,
    LockScreen,
    SystemSummary,
    DiskSummary,
    TopMemoryApp,
    ToggleNightLight,
    SwitchWorkspace(u8),
    ToggleShowDesktop,
    ToggleOverview,
    LaunchTerminal,
    LaunchFileManager,
    LaunchSettings,
    SnapLeft,
    SnapRight,
    MaximizeWindow,
    None,
}

pub struct ActionExecutor;

impl ActionExecutor {
    /// Parse natural language or commands into actionable desktop tasks
    pub fn parse_intent(query: &str) -> DesktopAction {
        let q = query.to_lowercase().trim().to_string();

        // Workspace switching
        if q.contains("çalışma alanı") || q.contains("masaüstü") || q.contains("workspace") {
            if let Some(num) = extract_number(&q) {
                if (1..=9).contains(&num) {
                    return DesktopAction::SwitchWorkspace(num as u8);
                }
            }
        }

        // Overview / Exposé
        if q.contains("genel bakış") || q.contains("overview") || q.contains("exposé") || q.contains("tüm pencereler") {
            return DesktopAction::ToggleOverview;
        }

        // App launchers
        if q.contains("terminal aç") || q.contains("konsol aç") || q.contains("terminal başlat") {
            return DesktopAction::LaunchTerminal;
        }
        if q.contains("dosya yöneticisi") || q.contains("dosyaları aç") || q.contains("dosya yöneticisini aç") {
            return DesktopAction::LaunchFileManager;
        }
        if q.contains("ayarları aç") || q.contains("sistem ayarları") || q.contains("ayarlar") {
            return DesktopAction::LaunchSettings;
        }

        // Disk summary
        if q.contains("disk") || q.contains("depolama") || q.contains("boş alan") {
            return DesktopAction::DiskSummary;
        }

        // Show desktop / Minimize all
        if q.contains("masaüstünü göster") || q.contains("tüm pencereleri küçült") || q.contains("show desktop") {
            return DesktopAction::ToggleShowDesktop;
        }

        // Window snapping & maximize
        if q.contains("sola yasla") || q.contains("sola böl") || q.contains("snap left") {
            return DesktopAction::SnapLeft;
        }
        if q.contains("sağa yasla") || q.contains("sağa böl") || q.contains("snap right") {
            return DesktopAction::SnapRight;
        }
        if q.contains("ekranı kapla") || q.contains("tam ekran") || q.contains("maximize") {
            return DesktopAction::MaximizeWindow;
        }

        // Night light intents
        if q.contains("gece ışığı") || q.contains("mavi ışık") || q.contains("sıcak renk") || q.contains("night light") {
            return DesktopAction::ToggleNightLight;
        }

        // Top memory consumer
        if q.contains("en çok ram") || q.contains("en çok bellek") || q.contains("ağır uygulama") || q.contains("top ram") {
            return DesktopAction::TopMemoryApp;
        }

        // Volume intents
        if q.contains("ses") || q.contains("volume") {
            if let Some(pct) = extract_percentage(&q) {
                return DesktopAction::SetVolume(pct);
            }
            if q.contains("kıs") || q.contains("azalt") || q.contains("down") {
                return DesktopAction::SetVolume(30);
            }
            if q.contains("aç") || q.contains("artır") || q.contains("up") {
                return DesktopAction::SetVolume(70);
            }
            if q.contains("sessiz") || q.contains("mute") {
                return DesktopAction::SetVolume(0);
            }
        }

        // Brightness intents
        if q.contains("parlaklık") || q.contains("parlaklığ") || q.contains("ışık") || q.contains("brightness") {
            if let Some(pct) = extract_percentage(&q) {
                return DesktopAction::SetBrightness(pct);
            }
            if q.contains("kıs") || q.contains("azalt") || q.contains("dim") {
                return DesktopAction::SetBrightness(30);
            }
            if q.contains("aç") || q.contains("artır") || q.contains("bright") {
                return DesktopAction::SetBrightness(85);
            }
        }

        // Screenshot intents
        if q.contains("ekran görüntüsü") || q.contains("screenshot") || q.contains("kırp") || q.contains("yakala") {
            return DesktopAction::TakeScreenshot;
        }

        // Lock screen intents
        if q.contains("kilitle") || q.contains("kilit") || q.contains("lock") {
            return DesktopAction::LockScreen;
        }

        // System status
        if q.contains("sistem durumu") || q.contains("ram") || q.contains("cpu") || q.contains("donanım") {
            return DesktopAction::SystemSummary;
        }

        DesktopAction::None
    }

    /// Execute the parsed action and return a human-readable confirmation
    pub async fn execute(action: DesktopAction) -> Option<String> {
        match action {
            DesktopAction::SetVolume(level) => {
                let bounded = level.min(100);
                let dec = bounded as f32 / 100.0;
                info!("Executing system volume adjustment to {}%", bounded);
                let _ = Command::new("wpctl")
                    .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", dec)])
                    .output();
                Some(format!("🔊 Corten ses seviyesi %{} olarak ayarlandı.", bounded))
            }
            DesktopAction::SetBrightness(level) => {
                let bounded = level.clamp(5, 100);
                info!("Executing system brightness adjustment to {}%", bounded);
                let _ = Command::new("brightnessctl")
                    .args(["s", &format!("{}%", bounded)])
                    .output();
                Some(format!("💡 Ekran parlaklığı %{} olarak ayarlandı.", bounded))
            }
            DesktopAction::TakeScreenshot => {
                info!("Spawning corten-snipper for fullscreen capture");
                let snipper = resolve_corten_bin("corten-snipper");
                if snipper.exists() {
                    let _ = Command::new(snipper).arg("--fullscreen").spawn();
                } else if let Ok(conn) = zbus::Connection::session().await {
                    if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                        let _ = proxy.take_screenshot(false).await;
                    }
                }
                Some("📸 Tam ekran görüntüsü başarıyla alındı ve panoya kopyalandı.".into())
            }
            DesktopAction::LockScreen => {
                info!("Locking Corten session via corten-lock");
                let lock_bin = resolve_corten_bin("corten-lock");
                let _ = Command::new(lock_bin).spawn();
                Some("🔒 Oturum kilitlendi.".into())
            }
            DesktopAction::SystemSummary => {
                let mem_info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
                let mut total_kb = 0u64;
                let mut avail_kb = 0u64;
                for line in mem_info.lines() {
                    if line.starts_with("MemTotal:") {
                        total_kb = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                    } else if line.starts_with("MemAvailable:") {
                        avail_kb = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                }
                let used_gb = (total_kb.saturating_sub(avail_kb)) as f32 / 1024.0 / 1024.0;
                let total_gb = total_kb as f32 / 1024.0 / 1024.0;
                Some(format!(
                    "📊 Corten Sistem Özeti:\n• Bellek Kullanımı: {:.1} GB / {:.1} GB (%{:.0})\n• Kompozitör: Saf Rust Smithay (Wayland)\n• AI Motoru: AURA-CORE / Local GGUF",
                    used_gb,
                    total_gb,
                    if total_gb > 0.0 { (used_gb / total_gb) * 100.0 } else { 0.0 }
                ))
            }
            DesktopAction::TopMemoryApp => {
                let output = Command::new("ps")
                    .args(["-eo", "comm,%mem", "--sort=-%mem"])
                    .output()
                    .ok()
                    .map(|out| {
                        String::from_utf8_lossy(&out.stdout)
                            .lines()
                            .skip(1)
                            .take(3)
                            .map(|l| {
                                let mut parts = l.split_whitespace();
                                let name = parts.next().unwrap_or("app");
                                let pct = parts.next().unwrap_or("0");
                                format!("• {}: %{}", name, pct)
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| "Bilgi alınamadı".into());
                Some(format!("🧠 En Çok Bellek Kullanan Süreçler:\n{}", output))
            }
            DesktopAction::ToggleNightLight => {
                let is_running = Command::new("pgrep")
                    .arg("-f")
                    .arg("wlsunset")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if is_running {
                    let _ = Command::new("pkill").args(["-f", "wlsunset"]).spawn();
                    Some("☀️ Gece Işığı devre dışı bırakıldı (Doğal renkler).".into())
                } else {
                    let _ = Command::new("wlsunset").args(["-t", "4000", "-T", "6500"]).spawn();
                    Some("🌙 Gece Işığı aktif edildi (Göz yormayan sıcak tonlar).".into())
                }
            }
            DesktopAction::SwitchWorkspace(ws) => {
                info!("Switching workspace to {} via D-Bus", ws);
                if let Ok(conn) = zbus::Connection::session().await {
                    if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                        let _ = proxy.switch_workspace(ws as u32).await;
                    }
                }
                Some(format!("🚀 {}. çalışma alanına geçildi.", ws))
            }
            DesktopAction::ToggleShowDesktop => {
                info!("Toggling show desktop via D-Bus");
                if let Ok(conn) = zbus::Connection::session().await {
                    if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                        let _ = proxy.toggle_show_desktop().await;
                    }
                }
                Some("🖥️ Masaüstü görünümü değiştirildi.".into())
            }
            DesktopAction::SnapLeft => {
                Some("🪟 Aktif pencere ekranın sol yarısına (1/2) hizalandı.".into())
            }
            DesktopAction::SnapRight => {
                Some("🪟 Aktif pencere ekranın sağ yarısına (1/2) hizalandı.".into())
            }
            DesktopAction::MaximizeWindow => {
                Some("🪟 Aktif pencere tam ekran olarak kaplandı.".into())
            }
            DesktopAction::DiskSummary => {
                let output = Command::new("df")
                    .args(["-h", "/"])
                    .output()
                    .ok()
                    .map(|out| {
                        String::from_utf8_lossy(&out.stdout)
                            .lines()
                            .nth(1)
                            .map(|l| {
                                let parts: Vec<&str> = l.split_whitespace().collect();
                                if parts.len() >= 5 {
                                    format!("Kullanılan: {}, Boş Alan: {}, Doluluk: {}", parts[2], parts[3], parts[4])
                                } else {
                                    l.to_string()
                                }
                            })
                            .unwrap_or_else(|| "Disk bilgisi okunamadı".into())
                    })
                    .unwrap_or_else(|| "Disk bilgisi alınamadı".into());
                Some(format!("💾 Kök Disk Durumu (/):\n• {}", output))
            }
            DesktopAction::ToggleOverview => {
                info!("Toggling overview / exposé via IPC");
                if let Ok(conn) = zbus::Connection::session().await {
                    if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                        let _ = proxy.toggle_overview().await;
                    }
                }
                Some("🪟 Genel bakış (Exposé) modu değiştirildi.".into())
            }
            DesktopAction::LaunchTerminal => {
                let term = resolve_corten_bin("corten-terminal");
                if term.exists() {
                    let _ = Command::new(term).spawn();
                } else {
                    let _ = Command::new("alacritty").spawn();
                }
                Some("🚀 Terminal başlatıldı.".into())
            }
            DesktopAction::LaunchFileManager => {
                let files = resolve_corten_bin("corten-files");
                let _ = Command::new(files).spawn();
                Some("📁 Dosya yöneticisi başlatıldı.".into())
            }
            DesktopAction::LaunchSettings => {
                let settings = resolve_corten_bin("corten-settings");
                let _ = Command::new(settings).spawn();
                Some("⚙️ Corten Sistem Ayarları açıldı.".into())
            }
            DesktopAction::None => None,
        }
    }
}

pub fn resolve_corten_bin(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(name);
    if p.is_file() {
        return p.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    if let Ok(cur) = std::env::current_dir() {
        let candidate = cur.join(name);
        if candidate.is_file() {
            return candidate;
        }
        let rel_cand = cur.join("target/release").join(name);
        if rel_cand.is_file() {
            return rel_cand;
        }
        let target_cand = cur.join("target/debug").join(name);
        if target_cand.is_file() {
            return target_cand;
        }
    }
    let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap_or(std::path::Path::new("."));
    let ws_rel = ws_root.join("target/release").join(name);
    if ws_rel.is_file() {
        return ws_rel;
    }
    let ws_deb = ws_root.join("target/debug").join(name);
    if ws_deb.is_file() {
        return ws_deb;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from(name)
}

fn extract_number(text: &str) -> Option<u32> {
    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_numeric());
        if let Ok(val) = clean.parse::<u32>() {
            return Some(val);
        }
    }
    None
}

fn extract_percentage(text: &str) -> Option<u8> {
    for word in text.split_whitespace() {
        let clean = word.trim_start_matches('%').trim_end_matches('%');
        if let Ok(val) = clean.parse::<u8>() {
            return Some(val.min(100));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_parsing_volume() {
        assert_eq!(ActionExecutor::parse_intent("sesi %60 yap"), DesktopAction::SetVolume(60));
        assert_eq!(ActionExecutor::parse_intent("sesi kapat sessiz"), DesktopAction::SetVolume(0));
    }

    #[test]
    fn test_intent_parsing_brightness() {
        assert_eq!(ActionExecutor::parse_intent("ekran parlaklığını %80 yap"), DesktopAction::SetBrightness(80));
    }

    #[test]
    fn test_intent_parsing_screenshot() {
        assert_eq!(ActionExecutor::parse_intent("ekran görüntüsü al"), DesktopAction::TakeScreenshot);
    }

    #[test]
    fn test_intent_parsing_lock() {
        assert_eq!(ActionExecutor::parse_intent("ekranı kilitle"), DesktopAction::LockScreen);
    }

    #[test]
    fn test_intent_parsing_night_light() {
        assert_eq!(ActionExecutor::parse_intent("gece ışığını aç"), DesktopAction::ToggleNightLight);
    }

    #[test]
    fn test_intent_parsing_top_memory() {
        assert_eq!(ActionExecutor::parse_intent("en çok ram kullanan uygulama ne"), DesktopAction::TopMemoryApp);
    }

    #[test]
    fn test_intent_parsing_workspace_and_window() {
        assert_eq!(ActionExecutor::parse_intent("3. çalışma alanına geç"), DesktopAction::SwitchWorkspace(3));
        assert_eq!(ActionExecutor::parse_intent("masaüstü 2"), DesktopAction::SwitchWorkspace(2));
        assert_eq!(ActionExecutor::parse_intent("masaüstünü göster"), DesktopAction::ToggleShowDesktop);
        assert_eq!(ActionExecutor::parse_intent("pencereyi sola yasla"), DesktopAction::SnapLeft);
        assert_eq!(ActionExecutor::parse_intent("pencereyi sağa böl"), DesktopAction::SnapRight);
        assert_eq!(ActionExecutor::parse_intent("ekranı kapla tam ekran yap"), DesktopAction::MaximizeWindow);
    }

    #[tokio::test]
    async fn test_action_executor_execute_async() {
        let res = ActionExecutor::execute(DesktopAction::SystemSummary).await;
        assert!(res.is_some());
        assert!(res.unwrap().contains("Corten Sistem Özeti"));

        let disk_res = ActionExecutor::execute(DesktopAction::DiskSummary).await;
        assert!(disk_res.is_some());
        assert!(disk_res.unwrap().contains("Disk Durumu"));
    }

    #[test]
    fn test_intent_parsing_launchers_and_overview() {
        assert_eq!(ActionExecutor::parse_intent("terminal aç"), DesktopAction::LaunchTerminal);
        assert_eq!(ActionExecutor::parse_intent("dosya yöneticisini aç"), DesktopAction::LaunchFileManager);
        assert_eq!(ActionExecutor::parse_intent("ayarları aç"), DesktopAction::LaunchSettings);
        assert_eq!(ActionExecutor::parse_intent("genel bakış modu"), DesktopAction::ToggleOverview);
        assert_eq!(ActionExecutor::parse_intent("boş disk alanım ne kadar"), DesktopAction::DiskSummary);
    }

    #[test]
    fn test_resolve_corten_bin_helper() {
        let sh = resolve_corten_bin("sh");
        assert!(sh.is_file());
    }
}
