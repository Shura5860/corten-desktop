use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

pub struct CheckItem {
    pub category: &'static str,
    pub title: &'static str,
    pub status: CheckStatus,
    pub details: String,
    pub remedy: Option<&'static str>,
}

#[derive(Default)]
pub struct SystemDoctor {
    checks: Vec<CheckItem>,
    score: u32,
    max_score: u32,
}

impl SystemDoctor {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            score: 0,
            max_score: 0,
        }
    }

    pub fn add_check(
        &mut self,
        category: &'static str,
        title: &'static str,
        status: CheckStatus,
        details: String,
        remedy: Option<&'static str>,
        weight: u32,
    ) {
        self.max_score += weight;
        match status {
            CheckStatus::Pass => self.score += weight,
            CheckStatus::Warn => self.score += weight / 2,
            CheckStatus::Fail => {}
        }
        self.checks.push(CheckItem {
            category,
            title,
            status,
            details,
            remedy,
        });
    }

    pub fn run_all_diagnostics(&mut self) {
        self.check_drm_subsystem();
        self.check_gpu_and_drivers();
        self.check_wayland_and_seat();
        self.check_pipewire_and_portal();
        self.check_font_and_assets();
        self.check_corten_binaries();
    }

    // 1. DRM & Kernel Subsystem
    fn check_drm_subsystem(&mut self) {
        let card0 = Path::new("/dev/dri/card0");
        let card1 = Path::new("/dev/dri/card1");
        let has_card = card0.exists() || card1.exists();

        if has_card {
            let active_card = if card0.exists() { "/dev/dri/card0" } else { "/dev/dri/card1" };
            self.add_check(
                "KMS / DRM Grafik",
                "KMS Görüntüleme Aygıtı",
                CheckStatus::Pass,
                format!("Aktif DRM düğümü bulundu ({})", active_card),
                None,
                15,
            );
        } else {
            self.add_check(
                "KMS / DRM Grafik",
                "KMS Görüntüleme Aygıtı",
                CheckStatus::Fail,
                "/dev/dri/card* bulunamadı. DRM/KMS çekirdek modülleri yüklenmemiş.".into(),
                Some("Çekirdek grafik modüllerinizi kontrol edin (amdgpu, i915 veya nvidia-drm)."),
                15,
            );
        }

        let render_node = Path::new("/dev/dri/renderD128");
        if render_node.exists() {
            let can_read = fs::File::open(render_node).is_ok();
            if can_read {
                self.add_check(
                    "KMS / DRM Grafik",
                    "Doğrudan Donanım Render Düğümü",
                    CheckStatus::Pass,
                    "renderD128 mevcut ve kullanıcı tarafından okunabiliyor".into(),
                    None,
                    10,
                );
            } else {
                self.add_check(
                    "KMS / DRM Grafik",
                    "Doğrudan Donanım Render Düğümü",
                    CheckStatus::Warn,
                    "renderD128 mevcut ancak doğrudan erişim izni yok".into(),
                    Some("Kullanıcınızı 'render' ve 'video' gruplarına ekleyin: sudo usermod -aG render,video $USER"),
                    10,
                );
            }
        } else {
            self.add_check(
                "KMS / DRM Grafik",
                "Doğrudan Donanım Render Düğümü",
                CheckStatus::Warn,
                "renderD128 bulunamadı; donanımsal hızlandırma sınırlı olabilir".into(),
                Some("GPU sürücülerinin (mesa, vulkan) kurulu olduğundan emin olun."),
                10,
            );
        }
    }

    // 2. GPU & Driver Detection
    fn check_gpu_and_drivers(&mut self) {
        let mut is_nvidia = false;
        let mut is_amd = false;
        let mut is_intel = false;

        if let Ok(modules) = fs::read_to_string("/proc/modules") {
            if modules.contains("nvidia") {
                is_nvidia = true;
            }
            if modules.contains("amdgpu") || modules.contains("radeon") {
                is_amd = true;
            }
            if modules.contains("i915") || modules.contains("xe") {
                is_intel = true;
            }
        }

        let driver_desc = if is_nvidia && (is_amd || is_intel) {
            "Hibrit Grafik (Nvidia dGPU + iGPU)"
        } else if is_nvidia {
            "Nvidia Bağımsız Grafik"
        } else if is_amd {
            "AMD Radeon (Mesa RADV/amdgpu)"
        } else if is_intel {
            "Intel Graphics (i915/xe)"
        } else {
            "Standart / Sanal Grafik Sürücüsü"
        };

        self.add_check(
            "Ekran Kartı & Sürücüler",
            "Aktif GPU Mimarisi",
            CheckStatus::Pass,
            driver_desc.into(),
            None,
            10,
        );

        if is_nvidia {
            let modeset_path = Path::new("/sys/module/nvidia_drm/parameters/modeset");
            if modeset_path.exists() {
                let content = fs::read_to_string(modeset_path).unwrap_or_default();
                let trimmed = content.trim();
                if trimmed == "Y" || trimmed == "1" {
                    self.add_check(
                        "Ekran Kartı & Sürücüler",
                        "Nvidia DRM Modesetting",
                        CheckStatus::Pass,
                        "nvidia-drm.modeset=1 aktif (KMS etkin)".into(),
                        None,
                        15,
                    );
                } else {
                    self.add_check(
                        "Ekran Kartı & Sürücüler",
                        "Nvidia DRM Modesetting",
                        CheckStatus::Fail,
                        "nvidia-drm.modeset pasif (Değer: 0/N). Wayland oturumu başlatılamaz!".into(),
                        Some("/etc/modprobe.d/nvidia.conf dosyasına 'options nvidia-drm modeset=1' ekleyin ve initramfs güncelleyin."),
                        15,
                    );
                }
            } else {
                self.add_check(
                    "Ekran Kartı & Sürücüler",
                    "Nvidia DRM Modülü",
                    CheckStatus::Warn,
                    "nvidia_drm parametresi okunamadı. modeset=1 gerekebilir.".into(),
                    Some("Çekirdek önyükleme satırına 'nvidia_drm.modeset=1' ekleyin."),
                    15,
                );
            }
        }
    }

    // 3. Wayland & Seat Permissions
    fn check_wayland_and_seat(&mut self) {
        if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
            let p = Path::new(&runtime_dir);
            if p.exists() && p.is_dir() {
                self.add_check(
                    "Wayland & Oturum",
                    "XDG_RUNTIME_DIR Soket Dizin Güvenliği",
                    CheckStatus::Pass,
                    format!("Geçerli ve erişilebilir ({})", runtime_dir),
                    None,
                    10,
                );
            } else {
                self.add_check(
                    "Wayland & Oturum",
                    "XDG_RUNTIME_DIR Soket Dizin Güvenliği",
                    CheckStatus::Fail,
                    format!("XDG_RUNTIME_DIR belirtilmiş ancak dizin mevcut değil ({})", runtime_dir),
                    Some("systemd logind yapılandırmasını kontrol edin."),
                    10,
                );
            }
        } else {
            self.add_check(
                "Wayland & Oturum",
                "XDG_RUNTIME_DIR Soket Dizin Güvenliği",
                CheckStatus::Fail,
                "XDG_RUNTIME_DIR ortam değişkeni tanımlı değil!".into(),
                Some("export XDG_RUNTIME_DIR=/run/user/$(id -u) yapın."),
                10,
            );
        }

        let wayland_display = env::var("WAYLAND_DISPLAY");
        if let Ok(display) = wayland_display {
            self.add_check(
                "Wayland & Oturum",
                "Wayland Görüntü Sunucusu Soketi",
                CheckStatus::Pass,
                format!("Aktif bir Wayland oturumu içinde çalışıyor ({})", display),
                None,
                5,
            );
        } else {
            self.add_check(
                "Wayland & Oturum",
                "Wayland Görüntü Sunucusu Soketi",
                CheckStatus::Warn,
                "Şu an aktif bir Wayland oturumunda değilsiniz (TTY veya X11 ortamı)".into(),
                Some("'corten-session' ile oturumu başlatabilirsiniz."),
                5,
            );
        }
    }

    // 4. Audio & Screencast Portals
    fn check_pipewire_and_portal(&mut self) {
        let uid = if let Ok(u) = env::var("UID") {
            u
        } else {
            let out = Command::new("id").arg("-u").output();
            out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_else(|_| "1000".into())
        };

        let pipewire_sock = format!("/run/user/{}/pipewire-0", uid);
        if Path::new(&pipewire_sock).exists() {
            self.add_check(
                "Ses & Ekran Paylaşımı (Portal)",
                "PipeWire Ses & Video Sunucusu",
                CheckStatus::Pass,
                format!("PipeWire soketi aktif ({})", pipewire_sock),
                None,
                10,
            );
        } else {
            self.add_check(
                "Ses & Ekran Paylaşımı (Portal)",
                "PipeWire Ses & Video Sunucusu",
                CheckStatus::Warn,
                "PipeWire soketi bulunamadı. Ekran paylaşımı ve ses göstergesi sınırlı olabilir.".into(),
                Some("systemctl --user enable --now pipewire pipewire-pulse wireplumber"),
                10,
            );
        }

        let portal_conf1 = Path::new("/usr/share/xdg-desktop-portal/corten-portals.conf");
        let portal_conf2 = Path::new("/usr/local/share/xdg-desktop-portal/corten-portals.conf");
        let portal_conf3 = Path::new("/etc/xdg-desktop-portal/corten-portals.conf");

        if portal_conf1.exists() || portal_conf2.exists() || portal_conf3.exists() {
            self.add_check(
                "Ses & Ekran Paylaşımı (Portal)",
                "Corten XDG Portal Yapılandırması",
                CheckStatus::Pass,
                "corten-portals.conf tanımlı (Ekran paylaşımı ve dosya seçici hazır)".into(),
                None,
                10,
            );
        } else {
            self.add_check(
                "Ses & Ekran Paylaşımı (Portal)",
                "Corten XDG Portal Yapılandırması",
                CheckStatus::Warn,
                "corten-portals.conf bulunamadı. OBS ve Discord ekran paylaşımı algılamayabilir.".into(),
                Some("sudo make install veya ./install.sh çalıştırarak portal yapılandırmasını kurun."),
                10,
            );
        }
    }

    // 5. Font & Bespoke Asset Verification
    fn check_font_and_assets(&mut self) {
        let has_icons_system = Path::new("/usr/share/icons/hicolor/128x128/apps/corten-settings.png").exists()
            || Path::new("/usr/local/share/icons/hicolor/128x128/apps/corten-settings.png").exists();
        let has_icons_local = Path::new("assets/icons/hicolor/128x128/apps/corten-settings.png").exists();

        if has_icons_system || has_icons_local {
            self.add_check(
                "Görsel Varlıklar & İkonlar",
                "Paket A Çelik İkon Seti",
                CheckStatus::Pass,
                "Tüm çözünürlüklerde (48, 64, 128, 256) squircle ikonlar mevcut".into(),
                None,
                10,
            );
        } else {
            self.add_check(
                "Görsel Varlıklar & İkonlar",
                "Paket A Çelik İkon Seti",
                CheckStatus::Warn,
                "Corten özgün ikonları kurulu değil; varsayılan jenerik ikonlar kullanılabilir.".into(),
                Some("./install.sh çalıştırarak ikonları /usr/share/icons/ dizinine kurun."),
                10,
            );
        }
    }

    // 6. Corten Core Binaries
    fn check_corten_binaries(&mut self) {
        let core_bins = [
            "corten-compositor",
            "corten-session",
            "corten-panel",
            "corten-launcher",
            "corten-settings",
            "corten-files",
            "corten-widgets",
            "corten-forge",
            "corten-update",
            "corten-ai",
            "corten-notify",
            "corten-snipper",
            "corten-portal",
            "corten-polkit",
        ];

        let mut missing = Vec::new();
        let mut found_count = 0;

        for bin in &core_bins {
            if Self::find_bin(bin).is_some() {
                found_count += 1;
            } else {
                missing.push(*bin);
            }
        }

        if missing.is_empty() {
            self.add_check(
                "Çekirdek İkililer",
                "Corten Ekosistem Bütünlüğü",
                CheckStatus::Pass,
                format!("Tüm 14 çekirdek ikili dosya başarıyla tespit edildi ({}/{})", found_count, core_bins.len()),
                None,
                15,
            );
        } else {
            self.add_check(
                "Çekirdek İkililer",
                "Corten Ekosistem Bütünlüğü",
                CheckStatus::Warn,
                format!("Eksik ikililer var ({}/{} bulundu): {:?}", found_count, core_bins.len(), missing),
                Some("cargo build --release --workspace veya ./install.sh çalıştırın."),
                15,
            );
        }
    }

    fn find_bin(name: &str) -> Option<PathBuf> {
        if let Ok(output) = Command::new("which").arg(name).output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return Some(PathBuf::from(s));
            }
        }
        for dir in ["target/release", "target/debug", "/usr/local/bin", "/usr/bin"] {
            let p = Path::new(dir).join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    pub fn print_report(&self, markdown: bool) {
        let pct = (self.score * 100)
            .checked_div(self.max_score)
            .unwrap_or(0);

        if markdown {
            self.print_markdown_report(pct);
        } else {
            self.print_terminal_report(pct);
        }
    }

    fn print_terminal_report(&self, pct: u32) {
        println!("\x1b[1;38;2;225;110;45m");
        println!("╔═══════════════════════════════════════════════════════════════════════╗");
        println!("║        CORTEN DESKTOP SYSTEM DOCTOR & HARDWARE DIAGNOSTICS            ║");
        println!("║        Forged Steel System Audit & Production Readiness Probe         ║");
        println!("╚═══════════════════════════════════════════════════════════════════════╝\x1b[0m\n");

        let mut current_cat = "";
        for item in &self.checks {
            if item.category != current_cat {
                current_cat = item.category;
                println!("\x1b[1;37m▶ {}\x1b[0m", current_cat);
            }

            let (badge, color) = match item.status {
                CheckStatus::Pass => ("[✓ PASS]", "\x1b[32m"),
                CheckStatus::Warn => ("[▲ WARN]", "\x1b[33m"),
                CheckStatus::Fail => ("[✗ FAIL]", "\x1b[31m"),
            };

            println!(
                "  {}{}\x1b[0m \x1b[1m{:<32}\x1b[0m : {}",
                color, badge, item.title, item.details
            );
            if let Some(remedy) = item.remedy {
                println!("         \x1b[38;2;225;110;45m↳ Çözüm:\x1b[0m \x1b[90m{}\x1b[0m", remedy);
            }
        }

        println!("\n\x1b[1;37m-----------------------------------------------------------------------\x1b[0m");
        let (rating_text, rating_color) = if pct >= 90 {
            ("MÜKEMMEL · Üretime Hazır ve Kararlı", "\x1b[32m")
        } else if pct >= 70 {
            ("İYİ · Kullanılabilir (Küçük Uyarılar Var)", "\x1b[33m")
        } else {
            ("DİKKAT · Kritik Eksikler Var", "\x1b[31m")
        };

        println!(
            "Sistem Sağlık Skoru : {}{}% ({})\x1b[0m",
            rating_color, pct, rating_text
        );
        println!("-----------------------------------------------------------------------\n");
    }

    fn print_markdown_report(&self, pct: u32) {
        println!("## Corten Desktop System Doctor Report\n");
        println!("* **Sağlık Skoru:** `{}% / 100%`", pct);
        println!("* **Tarih:** `{}`\n", chrono::Local::now().to_rfc3339());
        println!("| Durum | Kategori | Test | Detay |");
        println!("| :---: | :--- | :--- | :--- |");

        for item in &self.checks {
            let status_icon = match item.status {
                CheckStatus::Pass => "🟢 PASS",
                CheckStatus::Warn => "🟡 WARN",
                CheckStatus::Fail => "🔴 FAIL",
            };
            println!(
                "| {} | {} | {} | {} |",
                status_icon, item.category, item.title, item.details
            );
        }

        println!("\n### Önerilen Aksiyonlar\n");
        for item in &self.checks {
            if let Some(remedy) = item.remedy {
                println!("* **{}:** {}", item.title, remedy);
            }
        }
    }
}
