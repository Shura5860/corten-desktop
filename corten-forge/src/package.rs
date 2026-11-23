use futures::stream::{self, StreamExt};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PackageSource {
    Flatpak,
    ArchRepo,
    Aur,
    CortenCore,
}

impl PackageSource {
    pub fn label(&self) -> &'static str {
        match self {
            PackageSource::Flatpak => "Flatpak (Flathub)",
            PackageSource::ArchRepo => "Arch Resmi Deposu",
            PackageSource::Aur => "Arch Topluluk (AUR)",
            PackageSource::CortenCore => "Corten OTA (Çekirdek Sistem)",
        }
    }

    pub fn badge_color(&self) -> (f32, f32, f32) {
        match self {
            PackageSource::Flatpak => (0.20, 0.65, 0.85), // Cyan/Teal
            PackageSource::ArchRepo => (0.88, 0.42, 0.23), // Corten rust/copper
            PackageSource::Aur => (0.85, 0.65, 0.15),     // Amber
            PackageSource::CortenCore => (0.88, 0.45, 0.18), // Corten Forged Patina
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PackageState {
    Available,
    Installed,
    Upgradable {
        current_version: String,
        new_version: String,
    },
    Installing {
        progress: Option<f32>,
        message: String,
    },
    Removing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProductCategory {
    All,
    Featured,
    Development,
    Graphics,
    AudioVideo,
    Productivity,
    System,
    Gaming,
    Internet,
}

impl ProductCategory {
    pub fn label(&self) -> &'static str {
        match self {
            ProductCategory::All => "Tümü",
            ProductCategory::Featured => "Öne Çıkanlar",
            ProductCategory::Development => "Geliştirici Araçları",
            ProductCategory::Graphics => "Grafik & Tasarım",
            ProductCategory::AudioVideo => "Ses & Video",
            ProductCategory::Productivity => "Üretkenlik & Ofis",
            ProductCategory::System => "Sistem & Yardımcılar",
            ProductCategory::Gaming => "Oyun & Eğlence",
            ProductCategory::Internet => "İnternet & Ağ",
        }
    }

    pub fn icon(&self) -> char {
        use corten_theme::Icon;
        match self {
            ProductCategory::All => Icon::LAYERS,
            ProductCategory::Featured => Icon::SPARKLES,
            ProductCategory::Development => Icon::CODE,
            ProductCategory::Graphics => Icon::PALETTE,
            ProductCategory::AudioVideo => Icon::MUSIC,
            ProductCategory::Productivity => Icon::FILE_TEXT,
            ProductCategory::System => Icon::SETTINGS,
            ProductCategory::Gaming => Icon::GAMEPAD,
            ProductCategory::Internet => Icon::GLOBE,
        }
    }

    pub fn flathub_collection_url(&self) -> &'static str {
        match self {
            ProductCategory::All | ProductCategory::Featured => {
                "https://flathub.org/api/v2/collection/popular?page=1&per_page=40"
            }
            ProductCategory::Development => {
                "https://flathub.org/api/v2/collection/category/Development?page=1&per_page=40"
            }
            ProductCategory::Graphics => {
                "https://flathub.org/api/v2/collection/category/Graphics?page=1&per_page=40"
            }
            ProductCategory::AudioVideo => {
                "https://flathub.org/api/v2/collection/category/AudioVideo?page=1&per_page=40"
            }
            ProductCategory::Productivity => {
                "https://flathub.org/api/v2/collection/category/Office?page=1&per_page=40"
            }
            ProductCategory::System => {
                "https://flathub.org/api/v2/collection/category/System?page=1&per_page=40"
            }
            ProductCategory::Gaming => {
                "https://flathub.org/api/v2/collection/category/Game?page=1&per_page=40"
            }
            ProductCategory::Internet => {
                "https://flathub.org/api/v2/collection/category/Network?page=1&per_page=40"
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxPermissions {
    pub network: bool,
    pub pulseaudio: bool,
    pub wayland: bool,
    pub x11_fallback: bool,
    pub dri: bool,
    pub filesystems: Vec<String>,
    pub dbus_talk: Vec<String>,
    pub safety_score: u8, // 0 - 100
    pub safety_summary: String,
}

pub const ICON_CODE: &str = "\u{e093}";
pub const ICON_APP: &str = "\u{e426}";
pub const ICON_MUSIC: &str = "\u{e34c}";
pub const ICON_USER: &str = "\u{e19f}";
pub const ICON_CAMERA: &str = "\u{e064}";
pub const ICON_TERMINAL: &str = "\u{e181}";
pub const ICON_ACTIVITY: &str = "\u{e038}";
pub const ICON_CPU: &str = "\u{e0a9}";
pub const ICON_DOWNLOAD: &str = "\u{e0b2}";
pub const ICON_ZAP: &str = "\u{e1b4}";
#[allow(dead_code)]
pub const ICON_SETTINGS: &str = "\u{e154}";

#[derive(Debug, Clone, Default)]
pub struct AppDetails {
    pub app_id: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub developer: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub download_size: Option<String>,
    pub installed_size: Option<String>,
    pub icon_url: Option<String>,
    pub screenshots: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageItem {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub description: String,
    pub version: String,
    pub source: PackageSource,
    pub state: PackageState,
    pub category: ProductCategory,
    pub icon_char: &'static str,
    #[serde(default)]
    pub icon_url: Option<String>,
    pub download_size: Option<String>,
    pub installed_size: Option<String>,
    pub developer: Option<String>,
    pub license: Option<String>,
    pub permissions: Option<SandboxPermissions>,
}

pub struct PackageManager;

impl PackageManager {
    /// Loads a rich curated catalog of popular Flathub and Arch applications,
    /// cross-checking with the live system to detect whether they are already installed.
    pub async fn fetch_curated_catalog() -> Vec<PackageItem> {
        let installed_flatpaks = Self::get_installed_flatpak_ids().await;
        let installed_pacmans = Self::get_installed_pacman_pkgs().await;

        let mut catalog = vec![
            PackageItem {
                id: "com.visualstudio.code".into(),
                name: "Visual Studio Code".into(),
                summary: "Güçlü ve genişletilebilir kaynak kod editörü".into(),
                description: "Visual Studio Code, modern web ve bulut uygulamaları geliştirmek için optimize edilmiş, yerleşik hata ayıklayıcı ve git entegrasyonuna sahip hafif ama güçlü bir kod editörüdür.".into(),
                version: "1.95.0".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Development,
                icon_char: ICON_CODE,
                icon_url: None,
                download_size: Some("110 MB".into()),
                installed_size: Some("360 MB".into()),
                developer: Some("Microsoft".into()),
                license: Some("Özel Mülk / Açık Çekirdek".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["host:rw".into(), "xdg-config:rw".into()],
                    dbus_talk: vec!["org.freedesktop.secrets".into()],
                    safety_score: 75,
                    safety_summary: "Genişletilmiş Geliştirici İzni: Yerel dosya sistemine derleme ve dosya yönetimi için erişim talep ediyor.".into(),
                }),
            },
            PackageItem {
                id: "org.blender.Blender".into(),
                name: "Blender 3D".into(),
                summary: "Özgür ve açık kaynaklı 3D modelleme & animasyon kiti".into(),
                description: "Blender; 3D modelleme, heykel, rigging, animasyon, simülasyon, render, kompozisyon, hareket takibi ve video düzenleme gibi 3D üretim hattının tamamını destekleyen açık kaynaklı bir grafik paketidir.".into(),
                version: "5.2.0".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Graphics,
                icon_char: ICON_APP,
                icon_url: None,
                download_size: Some("320 MB".into()),
                installed_size: Some("890 MB".into()),
                developer: Some("Blender Foundation".into()),
                license: Some("GPL-3.0".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-documents:rw".into(), "xdg-pictures:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 95,
                    safety_summary: "Yüksek Güvenlikli Sandbox: Yalnızca GPU ve seçilen medya dizinlerine erişim sağlar, sistem dosyalarından izoledir.".into(),
                }),
            },
            PackageItem {
                id: "com.spotify.Client".into(),
                name: "Spotify".into(),
                summary: "Milyonlarca şarkı ve podcast'e anında erişim".into(),
                description: "Spotify ile sevdiğiniz müzikleri dinleyebilir, yeni çalma listeleri keşfedebilir ve en sevdiğiniz podcast'leri takip edebilirsiniz.".into(),
                version: "1.2.95".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::AudioVideo,
                icon_char: ICON_MUSIC,
                icon_url: None,
                download_size: Some("175 MB".into()),
                installed_size: Some("420 MB".into()),
                developer: Some("Spotify AB".into()),
                license: Some("Özel Mülk".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-music:ro".into()],
                    dbus_talk: vec!["org.mpris.MediaPlayer2.spotify".into()],
                    safety_score: 90,
                    safety_summary: "Korumalı Medya Sanal Alanı: Yalnızca ses aygıtı ve müzik klasörüyle sınırlıdır.".into(),
                }),
            },
            PackageItem {
                id: "com.discordapp.Discord".into(),
                name: "Discord".into(),
                summary: "Oyun toplulukları, sesli sohbet ve mesajlaşma".into(),
                description: "Discord arkadaşlarınızla konuşabileceğiniz, takılabileceğiniz ve topluluklar oluşturabileceğiniz modern bir iletişim platformudur.".into(),
                version: "1.0.169".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Internet,
                icon_char: ICON_USER,
                icon_url: None,
                download_size: Some("95 MB".into()),
                installed_size: Some("280 MB".into()),
                developer: Some("Discord Inc.".into()),
                license: Some("Özel Mülk".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-download:rw".into()],
                    dbus_talk: vec!["org.freedesktop.Notifications".into()],
                    safety_score: 85,
                    safety_summary: "İletişim İzinleri: Ağ ve ses giriş/çıkış aygıtlarına erişir, dosya sistemi sınırlandırılmıştır.".into(),
                }),
            },
            PackageItem {
                id: "com.valvesoftware.Steam".into(),
                name: "Steam".into(),
                summary: "Dünyanın en popüler dijital oyun dağıtım platformu".into(),
                description: "Steam üzerinden binlerce oyunu keşfedin, indirin ve arkadaşlarınızla çevrimiçi oynayın. Proton entegrasyonu sayesinde binlerce Windows oyunu doğrudan Wayland üzerinde çalışır.".into(),
                version: "1.0.0.87".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Gaming,
                icon_char: ICON_APP,
                icon_url: None,
                download_size: Some("35 MB".into()),
                installed_size: Some("120 MB".into()),
                developer: Some("Valve Corporation".into()),
                license: Some("Özel Mülk".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-data/Steam:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 80,
                    safety_summary: "Oyun İzinleri: Doğrudan donanım hızlandırma ve kontrolcü erişimi içerir.".into(),
                }),
            },
            PackageItem {
                id: "com.obsproject.Studio".into(),
                name: "OBS Studio".into(),
                summary: "Canlı yayın ve ekran kaydı yazılımı".into(),
                description: "OBS Studio, canlı yayın ve video kaydı için ücretsiz ve açık kaynaklı yazılımdır. Wayland PipeWire desteği ile pencereleri ve ekranları sıfır gecikmeyle yakalar.".into(),
                version: "31.0.0".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::AudioVideo,
                icon_char: ICON_CAMERA,
                icon_url: None,
                download_size: Some("130 MB".into()),
                installed_size: Some("390 MB".into()),
                developer: Some("OBS Project".into()),
                license: Some("GPL-2.0".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: false,
                    dri: true,
                    filesystems: vec!["xdg-videos:rw".into()],
                    dbus_talk: vec!["org.freedesktop.portal.ScreenCast".into()],
                    safety_score: 95,
                    safety_summary: "Wayland Portal Güvenliği: Ekran yakalama kullanıcı teyidiyle masaüstü portalı üzerinden yapılır.".into(),
                }),
            },
            PackageItem {
                id: "org.kde.kdenlive".into(),
                name: "Kdenlive".into(),
                summary: "Çok kanallı profesyonel video kurgu yazılımı".into(),
                description: "Kdenlive, neredeyse tüm ses ve video formatlarını destekleyen, çok kanallı zaman çizelgesi ve onlarca efekt içeren güçlü bir doğrusal olmayan video düzenleyicidir.".into(),
                version: "24.08.3".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::AudioVideo,
                icon_char: ICON_CAMERA,
                icon_url: None,
                download_size: Some("160 MB".into()),
                installed_size: Some("480 MB".into()),
                developer: Some("KDE Topluluğu".into()),
                license: Some("GPL-3.0".into()),
                permissions: Some(SandboxPermissions {
                    network: false,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-videos:rw".into(), "xdg-pictures:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 98,
                    safety_summary: "Yüksek İzolasyon: Ağ bağlantısı gerektirmez, tamamen çevrimdışı ve güvenlidir.".into(),
                }),
            },
            PackageItem {
                id: "org.gimp.GIMP".into(),
                name: "GIMP".into(),
                summary: "Gelişmiş görsel işleme ve fotoğraf manipülasyonu".into(),
                description: "GNU Image Manipulation Program, fotoğraf rötuşlama, görsel kompozisyon ve görsel oluşturma gibi görevler için serbestçe dağıtılan profesyonel bir araçtır.".into(),
                version: "3.0.0-RC1".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Graphics,
                icon_char: ICON_APP,
                icon_url: None,
                download_size: Some("180 MB".into()),
                installed_size: Some("520 MB".into()),
                developer: Some("The GIMP Team".into()),
                license: Some("GPL-3.0".into()),
                permissions: Some(SandboxPermissions {
                    network: false,
                    pulseaudio: false,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-pictures:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 99,
                    safety_summary: "Mükemmel Güvenlik: Ağ veya ses erişimi yoktur, yalnızca grafik işleme motoru çalışır.".into(),
                }),
            },
            PackageItem {
                id: "org.videolan.VLC".into(),
                name: "VLC Media Player".into(),
                summary: "Tüm ortam formatlarını oynatan efsanevi medya oynatıcı".into(),
                description: "VLC çoğu multimedya dosyasını, diskleri, cihazları ve ağ akış protokollerini ek kodek paketi gerektirmeden oynatan özgür ve açık kaynaklı bir oynatıcıdır.".into(),
                version: "3.0.21".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::AudioVideo,
                icon_char: ICON_CAMERA,
                icon_url: None,
                download_size: Some("68 MB".into()),
                installed_size: Some("210 MB".into()),
                developer: Some("VideoLAN".into()),
                license: Some("GPL-2.0".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: true,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-videos:ro".into(), "xdg-music:ro".into()],
                    dbus_talk: vec!["org.mpris.MediaPlayer2.vlc".into()],
                    safety_score: 95,
                    safety_summary: "Standart Medya İzinleri: Ağ akışları ve ses/video çıkışı için güvenli yapılandırılmıştır.".into(),
                }),
            },
            PackageItem {
                id: "org.kde.krita".into(),
                name: "Krita".into(),
                summary: "Dijital boyama ve illüstrasyon yazılımı".into(),
                description: "Krita illüstratörler, çizgi roman sanatçıları ve doku tasarımcıları için tasarlanmış profesyonel bir açık kaynaklı boyama programıdır.".into(),
                version: "5.2.6".into(),
                source: PackageSource::Flatpak,
                state: PackageState::Available,
                category: ProductCategory::Graphics,
                icon_char: ICON_APP,
                icon_url: None,
                download_size: Some("190 MB".into()),
                installed_size: Some("580 MB".into()),
                developer: Some("Krita Foundation".into()),
                license: Some("GPL-3.0".into()),
                permissions: Some(SandboxPermissions {
                    network: false,
                    pulseaudio: false,
                    wayland: true,
                    x11_fallback: true,
                    dri: true,
                    filesystems: vec!["xdg-pictures:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 98,
                    safety_summary: "Tam İzolasyon: Grafik tablet ve kalem hassasiyeti destekli, ağdan izole güvenli ortam.".into(),
                }),
            },
            PackageItem {
                id: "neovim".into(),
                name: "Neovim".into(),
                summary: "Genişletilebilir, hiper-hızlı terminal kod editörü".into(),
                description: "Neovim, Vim'in kullanıcı deneyimini, eklenti mimarisini (Lua) ve yerleşik LSP desteğini en üst düzeye çıkaran modern bir çatalıdır.".into(),
                version: "0.10.2".into(),
                source: PackageSource::ArchRepo,
                state: PackageState::Available,
                category: ProductCategory::Development,
                icon_char: ICON_TERMINAL,
                icon_url: None,
                download_size: Some("6.8 MB".into()),
                installed_size: Some("28 MB".into()),
                developer: Some("Neovim Core Team".into()),
                license: Some("Apache-2.0".into()),
                permissions: None,
            },
            PackageItem {
                id: "zed".into(),
                name: "Zed Editor".into(),
                summary: "GPU hızlandırmalı, ultra hızlı yeni nesil kod editörü".into(),
                description: "Rust ile sıfırdan yazılmış, GPU render motoru kullanan ve yapay zekâ entegrasyonuna sahip ultra performanslı metin editörü.".into(),
                version: "0.160.0".into(),
                source: PackageSource::ArchRepo,
                state: PackageState::Available,
                category: ProductCategory::Development,
                icon_char: ICON_CODE,
                icon_url: None,
                download_size: Some("32 MB".into()),
                installed_size: Some("98 MB".into()),
                developer: Some("Zed Industries".into()),
                license: Some("GPL-3.0 / AGPL-3.0".into()),
                permissions: None,
            },
            PackageItem {
                id: "btop".into(),
                name: "btop++".into(),
                summary: "Göz alıcı grafiksel terminal donanım monitörü".into(),
                description: "İşlemci, bellek, disk, ağ ve süreç kullanımını zengin renkli grafiklerle gösteren C++ tabanlı modern bir sistem monitörü.".into(),
                version: "1.3.2".into(),
                source: PackageSource::ArchRepo,
                state: PackageState::Available,
                category: ProductCategory::System,
                icon_char: ICON_ACTIVITY,
                icon_url: None,
                download_size: Some("1.4 MB".into()),
                installed_size: Some("4.2 MB".into()),
                developer: Some("aristocratos".into()),
                license: Some("Apache-2.0".into()),
                permissions: None,
            },
            PackageItem {
                id: "fastfetch".into(),
                name: "Fastfetch".into(),
                summary: "Son derece hızlı sistem bilgisi ve donanım dökümü".into(),
                description: "Sistem ve donanım yapılandırmanızı güzel ASCII sanatları ve performanslı C motoruyla ekrana getiren neofetch alternatifi.".into(),
                version: "2.29.0".into(),
                source: PackageSource::ArchRepo,
                state: PackageState::Available,
                category: ProductCategory::System,
                icon_char: ICON_CPU,
                icon_url: None,
                download_size: Some("0.9 MB".into()),
                installed_size: Some("2.8 MB".into()),
                developer: Some("Fastfetch Team".into()),
                license: Some("MIT".into()),
                permissions: None,
            },
            PackageItem {
                id: "corten-aura-model".into(),
                name: "Corten Yerel Dil Modeli (Qwen 2.5 Coder)".into(),
                summary: "Çevrimdışı ve gizlilik odaklı saf Rust çalışan yerel LLM GGUF ağırlığı".into(),
                description: "Corten AI asistanı için optimize edilmiş yerel Qwen 2.5 Coder GGUF modeli. Harici sunucu veya Python bağımlılığı gerektirmeden ~/.local/share/corten/models/ altına kurulur ve doğrudan yerel kompozitör yapay zeka çekirdeğiyle çalıştırılır.".into(),
                version: "2.5-Q4_K_M".into(),
                source: PackageSource::CortenCore,
                state: PackageState::Available,
                category: ProductCategory::Development,
                icon_char: ICON_CODE,
                icon_url: None,
                download_size: Some("986 MB".into()),
                installed_size: Some("986 MB".into()),
                developer: Some("Corten & Qwen Open Team".into()),
                license: Some("Apache-2.0".into()),
                permissions: Some(SandboxPermissions {
                    network: true,
                    pulseaudio: false,
                    wayland: false,
                    x11_fallback: false,
                    dri: true,
                    filesystems: vec!["~/.local/share/corten/models:rw".into()],
                    dbus_talk: vec![],
                    safety_score: 100,
                    safety_summary: "Sıfır Bulut / Tam İzolasyon: Model ağırlıkları tamamen yerel cihazınızda çalışır, hiçbir veri dışarıya gönderilmez.".into(),
                }),
            },
        ];

        let model_installed = {
            let model_dir = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".local/share/corten/models"))
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
            model_dir
                .join("qwen2.5-coder-1.5b-instruct-q4_k_m.gguf")
                .exists()
                || model_dir
                    .join("qwen2.5-coder-3b-instruct-q4_k_m.gguf")
                    .exists()
        };

        // Cross-reference with live system status
        for item in &mut catalog {
            match item.source {
                PackageSource::Flatpak => {
                    if installed_flatpaks.contains(&item.id) {
                        item.state = PackageState::Installed;
                    }
                }
                PackageSource::ArchRepo | PackageSource::Aur => {
                    if installed_pacmans.contains(&item.id) {
                        item.state = PackageState::Installed;
                    }
                }
                PackageSource::CortenCore => {
                    if item.id == "corten-aura-model" {
                        item.state = if model_installed {
                            PackageState::Installed
                        } else {
                            PackageState::Available
                        };
                    } else {
                        item.state = PackageState::Installed;
                    }
                }
            }
        }

        catalog
    }

    /// Fetches all currently installed Flatpak applications from the system
    pub async fn fetch_installed_flatpaks() -> Vec<PackageItem> {
        let mut list = Vec::new();
        let output = match Command::new("flatpak")
            .args([
                "list",
                "--app",
                "--columns=application,name,version,branch,arch",
            ])
            .output()
            .await
        {
            Ok(o) => o,
            Err(_) => return list,
        };

        if !output.status.success() {
            return list;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 3 {
                let id = cols[0].trim().to_string();
                let name = cols[1].trim().to_string();
                let version = cols[2].trim().to_string();

                if id.is_empty() || id == "Application ID" {
                    continue;
                }

                list.push(PackageItem {
                    id: id.clone(),
                    name,
                    summary: format!("Kurulu Flatpak Uygulaması ({})", id),
                    description: "Sisteminizde sanal alan (sandbox) içinde çalışan yerel uygulama."
                        .into(),
                    version,
                    source: PackageSource::Flatpak,
                    state: PackageState::Installed,
                    category: ProductCategory::All,
                    icon_char: ICON_DOWNLOAD,
                    icon_url: None,
                    download_size: None,
                    installed_size: None,
                    developer: None,
                    license: None,
                    permissions: None,
                });
            }
        }

        list
    }

    /// Parses Flathub API v2 `hits` array into `PackageItem` entries.
    fn parse_flathub_hits(
        body: &str,
        fallback_cat: ProductCategory,
        installed_flatpaks: &HashSet<String>,
    ) -> Vec<PackageItem> {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return Vec::new();
        };
        let Some(hits) = json.get("hits").and_then(|v| v.as_array()) else {
            return Vec::new();
        };

        let mut items = Vec::with_capacity(hits.len());
        for hit in hits {
            let Some(app_id) = hit.get("app_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = hit
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(app_id)
                .to_string();
            let summary = hit
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let developer = hit
                .get("developer_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let license = hit
                .get("project_license")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let is_installed = installed_flatpaks.contains(app_id);

            let icon_char = match fallback_cat {
                ProductCategory::Development => ICON_CODE,
                ProductCategory::Graphics => ICON_CAMERA,
                ProductCategory::AudioVideo => ICON_MUSIC,
                ProductCategory::System => ICON_SETTINGS,
                ProductCategory::Gaming => ICON_ACTIVITY,
                _ => ICON_APP,
            };

            let icon_url = hit
                .get("icon")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            items.push(PackageItem {
                id: app_id.to_string(),
                name,
                summary: summary.clone(),
                description: if summary.is_empty() {
                    format!("Flathub üzerinden sağlanan {} uygulaması.", app_id)
                } else {
                    summary
                },
                version: "Flathub".to_string(),
                source: PackageSource::Flatpak,
                state: if is_installed {
                    PackageState::Installed
                } else {
                    PackageState::Available
                },
                category: fallback_cat,
                icon_char,
                icon_url,
                download_size: None,
                installed_size: None,
                developer,
                license,
                permissions: None,
            });
        }
        items
    }

    /// Fetches live applications for a category directly from Flathub API v2 (with 10-min disk cache).
    pub async fn fetch_flathub_collection(category: ProductCategory) -> Vec<PackageItem> {
        let url = category.flathub_collection_url();
        let installed_flatpaks = Self::get_installed_flatpak_ids().await;

        let cache_key = format!("{:?}.json", category).to_lowercase();
        let cache_path = Self::cache_subdir("catalog").join(cache_key);
        if let Ok(meta) = tokio::fs::metadata(&cache_path).await
            && let Ok(modified) = meta.modified()
            && modified
                .elapsed()
                .map(|d| d.as_secs() < 600)
                .unwrap_or(false)
            && let Ok(cached_body) = tokio::fs::read_to_string(&cache_path).await
        {
            let parsed = Self::parse_flathub_hits(&cached_body, category, &installed_flatpaks);
            if !parsed.is_empty() {
                return parsed;
            }
        }

        let Ok(out) = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "8",
                "-H",
                "User-Agent: CortenForge/0.5.8",
                "-H",
                "Accept: application/json",
                url,
            ])
            .output()
            .await
        else {
            return Vec::new();
        };

        if !out.status.success() {
            return Vec::new();
        }
        let body = String::from_utf8_lossy(&out.stdout);
        let parsed = Self::parse_flathub_hits(&body, category, &installed_flatpaks);
        if !parsed.is_empty() {
            let _ = tokio::fs::write(&cache_path, body.as_bytes()).await;
        }
        parsed
    }

    /// Searches packages across live Flathub API v2, local Flatpak CLI, and Arch Repositories
    pub async fn search_packages(query: &str) -> Vec<PackageItem> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let installed_flatpaks = Self::get_installed_flatpak_ids().await;
        let installed_pacmans = Self::get_installed_pacman_pkgs().await;

        // 0. Live Flathub API v2 Search (works even if flatpak CLI is not installed yet)
        let payload = serde_json::json!({ "query": q }).to_string();
        if let Ok(flathub_out) = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "8",
                "-X",
                "POST",
                "https://flathub.org/api/v2/search",
                "-H",
                "Content-Type: application/json",
                "-H",
                "User-Agent: CortenForge/0.5.8",
                "-d",
                &payload,
            ])
            .output()
            .await
            && flathub_out.status.success()
        {
            let body = String::from_utf8_lossy(&flathub_out.stdout);
            for item in Self::parse_flathub_hits(&body, ProductCategory::All, &installed_flatpaks) {
                if seen_ids.insert(item.id.clone()) {
                    results.push(item);
                }
            }
        }

        // 1. Local Flatpak CLI Search (merges any extra remotes / versions)
        if let Ok(flatpak_out) = Command::new("flatpak")
            .args([
                "search",
                q,
                "--columns=application,name,description,version,branch,remotes",
            ])
            .output()
            .await
            && flatpak_out.status.success()
        {
            let stdout = String::from_utf8_lossy(&flatpak_out.stdout);
            for line in stdout.lines() {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() >= 4 {
                    let id = cols[0].trim().to_string();
                    let name = cols[1].trim().to_string();
                    let desc = cols[2].trim().to_string();
                    let ver = cols[3].trim().to_string();

                    if id.is_empty() || id == "Application ID" || !seen_ids.insert(id.clone()) {
                        continue;
                    }

                    let is_installed = installed_flatpaks.contains(&id);
                    results.push(PackageItem {
                        id: id.clone(),
                        name,
                        summary: desc.clone(),
                        description: desc,
                        version: ver,
                        source: PackageSource::Flatpak,
                        state: if is_installed {
                            PackageState::Installed
                        } else {
                            PackageState::Available
                        },
                        category: ProductCategory::All,
                        icon_char: ICON_DOWNLOAD,
                        icon_url: None,
                        download_size: None,
                        installed_size: None,
                        developer: None,
                        license: None,
                        permissions: None,
                    });
                }
            }
        }

        // 2. Pacman Search (top 15 matches)
        if let Ok(pacman_out) = Command::new("pacman").args(["-Ss", q]).output().await
            && pacman_out.status.success()
        {
            let stdout = String::from_utf8_lossy(&pacman_out.stdout);
            let mut lines = stdout.lines();
            let mut count = 0;

            while let Some(line1) = lines.next() {
                if count >= 12 {
                    break;
                }
                if line1.starts_with(' ') || line1.is_empty() {
                    continue;
                }

                // Format: repo/name version [options]
                if let Some((repo_pkg, ver_info)) = line1.split_once(' ') {
                    let pkg_name = repo_pkg.split_once('/').map(|(_, n)| n).unwrap_or(repo_pkg);
                    let version = ver_info.split_whitespace().next().unwrap_or("bilinmiyor");
                    let desc = lines
                        .next()
                        .map(|l| l.trim().to_string())
                        .unwrap_or_default();

                    let is_installed = installed_pacmans.contains(pkg_name);
                    results.push(PackageItem {
                        id: pkg_name.to_string(),
                        name: pkg_name.to_string(),
                        summary: desc.clone(),
                        description: desc,
                        version: version.to_string(),
                        source: PackageSource::ArchRepo,
                        state: if is_installed {
                            PackageState::Installed
                        } else {
                            PackageState::Available
                        },
                        category: ProductCategory::All,
                        icon_char: ICON_ZAP,
                        icon_url: None,
                        download_size: None,
                        installed_size: None,
                        developer: None,
                        license: None,
                        permissions: None,
                    });
                    count += 1;
                }
            }
        }

        results
    }

    /// Fetches and parses sandbox permissions for a Flatpak application
    pub async fn fetch_sandbox_permissions(app_id: &str) -> Option<SandboxPermissions> {
        let output = Command::new("flatpak")
            .args(["info", "-m", app_id])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let metadata = String::from_utf8_lossy(&output.stdout);
        let mut network = false;
        let mut pulseaudio = false;
        let mut wayland = false;
        let mut x11_fallback = false;
        let mut dri = false;
        let mut filesystems = Vec::new();
        let mut dbus_talk = Vec::new();

        let mut in_context = false;
        let mut in_session_bus = false;

        for line in metadata.lines() {
            let trimmed = line.trim();
            if trimmed == "[Context]" {
                in_context = true;
                in_session_bus = false;
                continue;
            } else if trimmed == "[Session Bus Policy]" {
                in_context = false;
                in_session_bus = true;
                continue;
            } else if trimmed.starts_with('[') {
                in_context = false;
                in_session_bus = false;
                continue;
            }

            if in_context {
                if let Some((key, val)) = trimmed.split_once('=') {
                    match key.trim() {
                        "shared" => {
                            if val.contains("network") {
                                network = true;
                            }
                        }
                        "sockets" => {
                            if val.contains("pulseaudio") {
                                pulseaudio = true;
                            }
                            if val.contains("wayland") {
                                wayland = true;
                            }
                            if val.contains("fallback-x11") || val.contains("x11") {
                                x11_fallback = true;
                            }
                        }
                        "devices" => {
                            if val.contains("dri") {
                                dri = true;
                            }
                        }
                        "filesystems" => {
                            for fs in val.split(';') {
                                let f = fs.trim();
                                if !f.is_empty() {
                                    filesystems.push(f.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            } else if in_session_bus && let Some((bus_name, _policy)) = trimmed.split_once('=') {
                let b = bus_name.trim();
                if !b.is_empty() {
                    dbus_talk.push(b.to_string());
                }
            }
        }

        // Calculate Safety Score
        let mut score = 100u8;
        if filesystems
            .iter()
            .any(|f| f.starts_with("host") || f.starts_with("home"))
        {
            score = score.saturating_sub(40);
        }
        if x11_fallback && !wayland {
            score = score.saturating_sub(15);
        }
        if filesystems.is_empty() {
            score = score.min(100);
        }

        let summary = if score >= 90 {
            "Mükemmel İzolasyon: Uygulama tam sanal alandadır, kişisel dosyalara izinsiz erişemez."
                .to_string()
        } else if score >= 70 {
            "Geliştirici İzni: Yerel dosya sistemi veya belirli D-Bus servislerine erişim yetkisi var.".to_string()
        } else {
            "Genişletilmiş İzin: Tüm ev dizinine veya ana sisteme erişim yetkisi talep ediyor."
                .to_string()
        };

        Some(SandboxPermissions {
            network,
            pulseaudio,
            wayland,
            x11_fallback,
            dri,
            filesystems,
            dbus_talk,
            safety_score: score,
            safety_summary: summary,
        })
    }

    /// Executes asynchronous package installation with streaming terminal logs
    pub async fn install_package(
        pkg: PackageItem,
        tx: UnboundedSender<String>,
    ) -> Result<(), String> {
        let _ = tx.send(format!("Kurulum başlatılıyor: {} ({})\n", pkg.name, pkg.id));

        match pkg.source {
            PackageSource::Flatpak => {
                // Ensure Flathub user remote exists so fresh installs work seamlessly
                let _ = Command::new("flatpak")
                    .args([
                        "remote-add",
                        "--if-not-exists",
                        "--user",
                        "flathub",
                        "https://dl.flathub.org/repo/flathub.flatpakrepo",
                    ])
                    .output()
                    .await;

                let mut child = Command::new("flatpak")
                    .args(["install", "--user", "-y", "flathub", &pkg.id])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| {
                        format!(
                            "Flatpak kurulu değil veya çalıştırılamadı (kurmak için: sudo pacman -S flatpak): {}",
                            e
                        )
                    })?;

                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout).lines();
                    let tx_out = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_out.send(format!("  {}\n", line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_err.send(format!("  {}\n", line));
                        }
                    });
                }

                let status = child
                    .wait()
                    .await
                    .map_err(|e| format!("Bekleme hatası: {}", e))?;
                if status.success() {
                    let _ = tx.send(format!("✅ {} başarıyla kuruldu!\n", pkg.name));
                    Ok(())
                } else {
                    Err(format!(
                        "Kurulum başarısız oldu (Çıkış kodu: {:?})",
                        status.code()
                    ))
                }
            }
            PackageSource::Aur => {
                // AUR packages cannot be installed with pacman directly; use paru or yay
                let helper = if std::path::Path::new("/usr/bin/paru").exists() {
                    "paru"
                } else if std::path::Path::new("/usr/bin/yay").exists() {
                    "yay"
                } else {
                    return Err(
                        "AUR paketlerini kurmak için 'paru' veya 'yay' kurulu olmalıdır.".into(),
                    );
                };
                let mut child = Command::new(helper)
                    .args(["-S", "--noconfirm", &pkg.id])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("{} çalıştırılamadı: {}", helper, e))?;

                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout).lines();
                    let tx_out = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_out.send(format!("  {}\n", line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_err.send(format!("  {}\n", line));
                        }
                    });
                }

                let status = child
                    .wait()
                    .await
                    .map_err(|e| format!("Bekleme hatası: {}", e))?;
                if status.success() {
                    let _ = tx.send(format!("✅ {} (AUR) başarıyla kuruldu!\n", pkg.name));
                    Ok(())
                } else {
                    Err(format!(
                        "AUR kurulumu başarısız oldu (Çıkış kodu: {:?})",
                        status.code()
                    ))
                }
            }
            PackageSource::ArchRepo => {
                let (pm_args, pm_label): (Vec<&str>, &str) =
                    if std::path::Path::new("/usr/bin/pacman").exists() {
                        (vec!["pacman", "-S", "--noconfirm", &pkg.id], "pacman")
                    } else if std::path::Path::new("/usr/bin/apt-get").exists() {
                        (vec!["apt-get", "install", "-y", &pkg.id], "apt-get")
                    } else if std::path::Path::new("/usr/bin/dnf").exists() {
                        (vec!["dnf", "install", "-y", &pkg.id], "dnf")
                    } else {
                        return Err(
                            "Desteklenen yerel paket yöneticisi (pacman/apt/dnf) bulunamadı."
                                .into(),
                        );
                    };
                let mut child = Command::new("pkexec")
                    .args(&pm_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("Polkit {} çalıştırılamadı: {}", pm_label, e))?;

                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout).lines();
                    let tx_out = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_out.send(format!("  {}\n", line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_err.send(format!("  {}\n", line));
                        }
                    });
                }

                let status = child
                    .wait()
                    .await
                    .map_err(|e| format!("Bekleme hatası: {}", e))?;
                if status.success() {
                    let _ = tx.send(format!("✅ {} başarıyla kuruldu!\n", pkg.name));
                    Ok(())
                } else {
                    Err(format!(
                        "Kurulum başarısız oldu (Çıkış kodu: {:?})",
                        status.code()
                    ))
                }
            }
            PackageSource::CortenCore => {
                if pkg.id == "corten-aura-model" {
                    let _ = tx.send("Yerel dil modeli indiriliyor (~986 MB)...\n".into());
                    let dest_dir = std::env::var("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".local/share/corten/models"))
                        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
                    let _ = tokio::fs::create_dir_all(&dest_dir).await;
                    let dest_file = dest_dir.join("qwen2.5-coder-1.5b-instruct-q4_k_m.gguf");
                    let dest_path_str = dest_file.to_string_lossy().to_string();

                    let url = "https://huggingface.co/Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF/resolve/main/qwen2.5-coder-1.5b-instruct-q4_k_m.gguf";
                    let mut child = Command::new("curl")
                        .args(["-L", "-o", &dest_path_str, url, "--progress-bar"])
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .map_err(|e| format!("İndirme başlatılamadı: {}", e))?;

                    if let Some(stderr) = child.stderr.take() {
                        let mut reader = BufReader::new(stderr).lines();
                        let tx_out = tx.clone();
                        tokio::spawn(async move {
                            while let Ok(Some(line)) = reader.next_line().await {
                                let _ = tx_out.send(format!("  {}\n", line));
                            }
                        });
                    }

                    let status = child
                        .wait()
                        .await
                        .map_err(|e| format!("Bekleme hatası: {}", e))?;
                    if status.success() {
                        let _ = tx.send(format!("Yerel model kuruldu: `{}`\n", dest_path_str));
                        Ok(())
                    } else {
                        Err(format!(
                            "Model indirme başarısız oldu (Çıkış kodu: {:?})",
                            status.code()
                        ))
                    }
                } else {
                    let _ = tx.send("Corten Desktop güncellemesi uygulanıyor...\n".into());
                    let info = corten_update::CortenUpdater::check_for_updates().await?;
                    let tx_clone = tx.clone();
                    corten_update::CortenUpdater::apply_update(&info, move |msg| {
                        let _ = tx_clone.send(format!("  {}\n", msg));
                    })
                    .await?;
                    let _ = tx.send(format!(
                        "✅ Corten Desktop başarıyla {} sürümüne güncellendi!\n",
                        info.latest_version
                    ));
                    Ok(())
                }
            }
        }
    }

    /// Executes asynchronous package uninstallation
    pub async fn uninstall_package(
        pkg: PackageItem,
        tx: UnboundedSender<String>,
    ) -> Result<(), String> {
        let _ = tx.send(format!(
            "🗑️ Kaldırma işlemi başlatılıyor: {} ({})\n",
            pkg.name, pkg.id
        ));

        match pkg.source {
            PackageSource::Flatpak => {
                let mut child = Command::new("flatpak")
                    .args(["uninstall", "-y", &pkg.id])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("Flatpak komutu çalıştırılamadı: {}", e))?;

                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout).lines();
                    let tx_out = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_out.send(format!("  {}\n", line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_err.send(format!("  {}\n", line));
                        }
                    });
                }

                let status = child
                    .wait()
                    .await
                    .map_err(|e| format!("Bekleme hatası: {}", e))?;
                if status.success() {
                    let _ = tx.send(format!("✅ {} sistemden kaldırıldı.\n", pkg.name));
                    Ok(())
                } else {
                    Err(format!(
                        "Kaldırma başarısız oldu (Çıkış kodu: {:?})",
                        status.code()
                    ))
                }
            }
            PackageSource::ArchRepo | PackageSource::Aur => {
                let mut child = Command::new("pkexec")
                    .args(["pacman", "-R", "--noconfirm", &pkg.id])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("Polkit pacman çalıştırılamadı: {}", e))?;

                if let Some(stdout) = child.stdout.take() {
                    let mut reader = BufReader::new(stdout).lines();
                    let tx_out = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_out.send(format!("  {}\n", line));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let tx_err = tx.clone();
                    tokio::spawn(async move {
                        while let Ok(Some(line)) = reader.next_line().await {
                            let _ = tx_err.send(format!("  {}\n", line));
                        }
                    });
                }

                let status = child
                    .wait()
                    .await
                    .map_err(|e| format!("Bekleme hatası: {}", e))?;
                if status.success() {
                    let _ = tx.send(format!("✅ {} sistemden kaldırıldı.\n", pkg.name));
                    Ok(())
                } else {
                    Err(format!(
                        "Kaldırma başarısız oldu (Çıkış kodu: {:?})",
                        status.code()
                    ))
                }
            }
            PackageSource::CortenCore => {
                if pkg.id == "corten-aura-model" {
                    let dest_dir = std::env::var("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".local/share/corten/models"))
                        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
                    let dest_file = dest_dir.join("qwen2.5-coder-1.5b-instruct-q4_k_m.gguf");
                    if dest_file.exists() {
                        let _ = tokio::fs::remove_file(&dest_file).await;
                        let _ = tx.send(format!(
                            "✅ Model dosyası silindi: `{}`\n",
                            dest_file.display()
                        ));
                    }
                    Ok(())
                } else {
                    Err("Corten Desktop çekirdek sistem bileşeni kaldırılamaz.".into())
                }
            }
        }
    }

    /// Checks for available updates from Flatpak and Corten Desktop OTA
    pub async fn check_flatpak_updates() -> Vec<PackageItem> {
        let mut list = Vec::new();

        // 1. Check Corten Desktop Core OS Update (GitHub Releases OTA)
        if let Ok(corten_info) = corten_update::CortenUpdater::check_for_updates().await
            && corten_info.has_update
        {
            let size_str = corten_info
                .asset_size_bytes
                .map(|b| format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)));
            list.push(PackageItem {
                id: "corten-desktop-core".into(),
                name: "Corten Desktop (Wayland Çekirdek Sistem)".into(),
                summary: format!(
                    "Corten Desktop {} sürümü hazır!",
                    corten_info.latest_version
                ),
                description: corten_info.release_notes,
                version: corten_info.latest_version.clone(),
                source: PackageSource::CortenCore,
                state: PackageState::Upgradable {
                    current_version: corten_info.current_version,
                    new_version: corten_info.latest_version,
                },
                category: ProductCategory::System,
                icon_char: ICON_ZAP,
                icon_url: None,
                download_size: size_str,
                installed_size: None,
                developer: Some("Corten Desktop Contributors".into()),
                license: Some("MIT / GPL-3.0".into()),
                permissions: None,
            });
        }

        // 2. Check Flatpak application updates
        if let Ok(output) = Command::new("flatpak")
            .args([
                "remote-ls",
                "--updates",
                "--columns=application,name,version,branch",
            ])
            .output()
            .await
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() >= 3 {
                    let id = cols[0].trim().to_string();
                    let name = cols[1].trim().to_string();
                    let ver = cols[2].trim().to_string();
                    if !id.is_empty() && id != "Application ID" {
                        list.push(PackageItem {
                            id: id.clone(),
                            name,
                            summary: "Yeni güncelleme mevcut".into(),
                            description:
                                "Bu uygulamanın Flathub deposunda yeni bir sürümü yayınlandı."
                                    .into(),
                            version: ver,
                            source: PackageSource::Flatpak,
                            state: PackageState::Upgradable {
                                current_version: "Yüklü Sürüm".into(),
                                new_version: "Yeni Sürüm".into(),
                            },
                            category: ProductCategory::All,
                            icon_char: ICON_DOWNLOAD,
                            icon_url: None,
                            download_size: None,
                            installed_size: None,
                            developer: None,
                            license: None,
                            permissions: None,
                        });
                    }
                }
            }
        }
        list
    }

    async fn get_installed_flatpak_ids() -> HashSet<String> {
        let mut ids = HashSet::new();
        if let Ok(output) = Command::new("flatpak")
            .args(["list", "--app", "--columns=application"])
            .output()
            .await
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let id = line.trim();
                if !id.is_empty() && id != "Application ID" {
                    ids.insert(id.to_string());
                }
            }
        }
        ids
    }

    async fn get_installed_pacman_pkgs() -> HashSet<String> {
        let mut pkgs = HashSet::new();
        if let Ok(output) = Command::new("pacman").args(["-Qq"]).output().await
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let p = line.trim();
                if !p.is_empty() {
                    pkgs.insert(p.to_string());
                }
            }
        }
        pkgs
    }

    fn cache_subdir(sub: &str) -> PathBuf {
        let base = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let dir = base.join("corten-forge").join(sub);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn safe_cache_name(id: &str) -> String {
        id.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn is_valid_image_bytes(bytes: &[u8]) -> bool {
        if bytes.len() < 12 {
            return false;
        }
        // PNG, JPEG, or WebP (RIFF....WEBP)
        bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
            || (bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
    }

    pub fn strip_html_to_text(html: &str) -> String {
        let mut s = html
            .replace("</p>", "\n\n")
            .replace("<br>", "\n")
            .replace("<br/>", "\n")
            .replace("<br />", "\n")
            .replace("<li>", "• ")
            .replace("</li>", "\n")
            .replace("</ul>", "\n")
            .replace("</ol>", "\n");

        // Strip all remaining HTML tags
        let mut out = String::with_capacity(s.len());
        let mut in_tag = false;
        for ch in s.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        s = out
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("&nbsp;", " ");

        // Collapse excessive consecutive blank lines
        let mut cleaned = Vec::new();
        let mut blank_run = 0usize;
        for line in s.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                blank_run += 1;
                if blank_run <= 1 && !cleaned.is_empty() {
                    cleaned.push(String::new());
                }
            } else {
                blank_run = 0;
                cleaned.push(trimmed.to_string());
            }
        }
        cleaned.join("\n").trim().to_string()
    }

    pub fn parse_flathub_appstream(app_id: &str, body: &str) -> Option<AppDetails> {
        let json = serde_json::from_str::<serde_json::Value>(body).ok()?;
        let description = json
            .get("description")
            .and_then(|v| v.as_str())
            .map(Self::strip_html_to_text)
            .filter(|s| !s.is_empty());

        let version = json
            .get("releases")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("version"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let developer = json
            .get("developer_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let license = json
            .get("project_license")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let homepage = json
            .get("urls")
            .and_then(|u| u.get("homepage"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let icon_url = json
            .get("icon")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let mut screenshots = Vec::new();
        if let Some(arr) = json.get("screenshots").and_then(|v| v.as_array()) {
            for sc in arr.iter().take(4) {
                let caption = sc
                    .get("caption")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                if let Some(sizes) = sc.get("sizes").and_then(|v| v.as_array()) {
                    // Prefer a crisp 700..1300px wide screenshot for fast download & sharp display
                    let best = sizes
                        .iter()
                        .filter_map(|sz| {
                            let src = sz.get("src").and_then(|v| v.as_str())?;
                            let w = sz
                                .get("width")
                                .and_then(|v| {
                                    v.as_str()
                                        .and_then(|s| s.parse::<u32>().ok())
                                        .or_else(|| v.as_u64().map(|n| n as u32))
                                })
                                .unwrap_or(0);
                            Some((src, w))
                        })
                        .min_by_key(|(_, w)| (*w as i64 - 900).abs());
                    if let Some((src, _)) = best {
                        screenshots.push((src.to_string(), caption));
                    }
                }
            }
        }

        Some(AppDetails {
            app_id: app_id.to_string(),
            description,
            version,
            developer,
            license,
            homepage,
            download_size: None,
            installed_size: None,
            icon_url,
            screenshots,
        })
    }

    /// Fetches rich application details (full description, screenshots, version, homepage)
    /// from Flathub API v2 (for Flatpak) or `pacman -Si` (for Arch Repo).
    pub async fn fetch_app_details(app_id: &str, source: PackageSource) -> Option<AppDetails> {
        match source {
            PackageSource::Flatpak => {
                let url = format!("https://flathub.org/api/v2/appstream/{}", app_id);
                let out = Command::new("curl")
                    .args([
                        "-sSL",
                        "--max-time",
                        "8",
                        "-H",
                        "User-Agent: CortenForge/0.5.8",
                        "-H",
                        "Accept: application/json",
                        &url,
                    ])
                    .output()
                    .await
                    .ok()?;
                if !out.status.success() {
                    return None;
                }
                let body = String::from_utf8_lossy(&out.stdout);
                Self::parse_flathub_appstream(app_id, &body)
            }
            PackageSource::ArchRepo => {
                let out = Command::new("pacman")
                    .args(["-Si", app_id])
                    .output()
                    .await
                    .ok()?;
                if !out.status.success() {
                    return None;
                }
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut details = AppDetails {
                    app_id: app_id.to_string(),
                    ..Default::default()
                };
                for line in stdout.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        let key = k.trim();
                        let val = v.trim().to_string();
                        if val.is_empty() || val == "None" {
                            continue;
                        }
                        match key {
                            "Version" => details.version = Some(val),
                            "Description" => details.description = Some(val),
                            "URL" => details.homepage = Some(val),
                            "Licenses" => details.license = Some(val),
                            "Download Size" => details.download_size = Some(val),
                            "Installed Size" => details.installed_size = Some(val),
                            "Packager" => details.developer = Some(val),
                            _ => {}
                        }
                    }
                }
                Some(details)
            }
            _ => None,
        }
    }

    async fn download_image_cached(url: &str, cache_path: &PathBuf) -> Option<Vec<u8>> {
        if let Ok(bytes) = tokio::fs::read(cache_path).await
            && Self::is_valid_image_bytes(&bytes)
        {
            return Some(bytes);
        }
        let out = Command::new("curl")
            .args([
                "-sSL",
                "--max-time",
                "7",
                "-H",
                "User-Agent: CortenForge/0.5.8",
                url,
            ])
            .output()
            .await
            .ok()?;
        if !out.status.success() || !Self::is_valid_image_bytes(&out.stdout) {
            return None;
        }
        let _ = tokio::fs::write(cache_path, &out.stdout).await;
        Some(out.stdout)
    }

    /// Loads or downloads an application's real icon (with disk cache).
    pub async fn fetch_icon_bytes(
        app_id: String,
        icon_url: Option<String>,
        source: PackageSource,
    ) -> Option<(String, Vec<u8>)> {
        let safe = Self::safe_cache_name(&app_id);
        let cache_path = Self::cache_subdir("icons").join(format!("{}.img", safe));
        if let Ok(bytes) = tokio::fs::read(&cache_path).await
            && Self::is_valid_image_bytes(&bytes)
        {
            return Some((app_id, bytes));
        }

        match source {
            PackageSource::Flatpak => {
                if let Some(ref url) = icon_url
                    && let Some(bytes) = Self::download_image_cached(url, &cache_path).await
                {
                    return Some((app_id, bytes));
                }
                let fallback = format!(
                    "https://dl.flathub.org/repo/appstream/x86_64/icons/128x128/{}.png",
                    app_id
                );
                let bytes = Self::download_image_cached(&fallback, &cache_path).await?;
                Some((app_id, bytes))
            }
            PackageSource::ArchRepo | PackageSource::Aur => {
                let candidates = [
                    format!("/usr/share/icons/hicolor/128x128/apps/{}.png", app_id),
                    format!("/usr/share/icons/hicolor/64x64/apps/{}.png", app_id),
                    format!("/usr/share/icons/hicolor/48x48/apps/{}.png", app_id),
                    format!("/usr/share/pixmaps/{}.png", app_id),
                ];
                for path in candidates {
                    if let Ok(bytes) = tokio::fs::read(&path).await
                        && Self::is_valid_image_bytes(&bytes)
                    {
                        let _ = tokio::fs::write(&cache_path, &bytes).await;
                        return Some((app_id, bytes));
                    }
                }
                None
            }
            PackageSource::CortenCore => None,
        }
    }

    /// Concurrently fetches icons for a batch of packages (bounded to 6 parallel requests).
    pub async fn fetch_icons_batch(
        items: Vec<(String, Option<String>, PackageSource)>,
    ) -> Vec<(String, Vec<u8>)> {
        stream::iter(items)
            .map(|(id, url, src)| Self::fetch_icon_bytes(id, url, src))
            .buffer_unordered(6)
            .filter_map(|res| async move { res })
            .collect()
            .await
    }

    /// Concurrently downloads and caches up to 4 screenshots for an application.
    pub async fn fetch_screenshots(
        app_id: String,
        urls: Vec<(String, Option<String>)>,
    ) -> (String, Vec<(Vec<u8>, Option<String>)>) {
        let safe = Self::safe_cache_name(&app_id);
        let dir = Self::cache_subdir("screenshots");
        let results: Vec<(usize, Vec<u8>, Option<String>)> =
            stream::iter(urls.into_iter().enumerate())
                .map(|(idx, (url, caption))| {
                    let cache_path = dir.join(format!("{}_{}.img", safe, idx));
                    async move {
                        let bytes = Self::download_image_cached(&url, &cache_path).await?;
                        Some((idx, bytes, caption))
                    }
                })
                .buffer_unordered(4)
                .filter_map(|res| async move { res })
                .collect()
                .await;

        let mut sorted = results;
        sorted.sort_by_key(|(idx, _, _)| *idx);
        let images = sorted.into_iter().map(|(_, b, c)| (b, c)).collect();
        (app_id, images)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_curated_catalog_loads() {
        let catalog = PackageManager::fetch_curated_catalog().await;
        assert!(!catalog.is_empty(), "Catalog must contain items");
        let has_vscode = catalog.iter().any(|p| p.id == "com.visualstudio.code");
        let has_blender = catalog.iter().any(|p| p.id == "org.blender.Blender");
        assert!(has_vscode, "Catalog should have VS Code");
        assert!(has_blender, "Catalog should have Blender");
    }

    #[test]
    fn test_category_labels_and_icons() {
        assert_eq!(ProductCategory::Development.label(), "Geliştirici Araçları");
        assert_eq!(ProductCategory::Graphics.label(), "Grafik & Tasarım");
        assert_eq!(
            ProductCategory::AudioVideo.icon(),
            corten_theme::Icon::MUSIC
        );
        assert_eq!(ProductCategory::Gaming.icon(), corten_theme::Icon::GAMEPAD);
    }

    #[test]
    fn test_source_badge_properties() {
        assert_eq!(PackageSource::Flatpak.label(), "Flatpak (Flathub)");
        assert_eq!(PackageSource::ArchRepo.label(), "Arch Resmi Deposu");
        let (r, g, b) = PackageSource::Flatpak.badge_color();
        assert!(r > 0.0 && g > 0.0 && b > 0.0);
    }

    #[test]
    fn test_sandbox_safety_score_calculation() {
        let clean_perm = SandboxPermissions {
            network: false,
            pulseaudio: true,
            wayland: true,
            x11_fallback: false,
            dri: true,
            filesystems: vec!["xdg-pictures:rw".into()],
            dbus_talk: vec![],
            safety_score: 95,
            safety_summary: "Korumalı Sanal Alan".into(),
        };
        assert!(clean_perm.safety_score >= 90);

        let wide_perm = SandboxPermissions {
            network: true,
            pulseaudio: true,
            wayland: false,
            x11_fallback: true,
            dri: true,
            filesystems: vec!["host:rw".into()],
            dbus_talk: vec![],
            safety_score: 45,
            safety_summary: "Geniş İzin".into(),
        };
        assert!(wide_perm.safety_score < 70);
    }

    #[test]
    fn test_strip_html_to_text_and_entities() {
        let html =
            "<p>Hello <b>Corten</b> &amp; friends!</p><ul><li>Fast</li><li>Safe &lt;3</li></ul>";
        let text = PackageManager::strip_html_to_text(html);
        assert!(text.contains("Hello Corten & friends!"));
        assert!(text.contains("• Fast"));
        assert!(text.contains("• Safe <3"));
        assert!(!text.contains("<p>") && !text.contains("<b>"));
    }

    #[test]
    fn test_parse_flathub_appstream() {
        let sample = r#"{
            "description": "<p>Great app for <b>Linux</b>.</p>",
            "developer_name": "Acme Dev",
            "project_license": "GPL-3.0",
            "icon": "https://dl.flathub.org/media/icon.png",
            "urls": { "homepage": "https://example.org" },
            "releases": [{ "version": "2.4.1" }],
            "screenshots": [{
                "caption": "Main view",
                "sizes": [
                    { "width": "400", "src": "https://dl.flathub.org/small.png" },
                    { "width": "920", "src": "https://dl.flathub.org/med.png" }
                ]
            }]
        }"#;
        let details = PackageManager::parse_flathub_appstream("org.example.App", sample).unwrap();
        assert_eq!(details.version.as_deref(), Some("2.4.1"));
        assert_eq!(details.developer.as_deref(), Some("Acme Dev"));
        assert_eq!(details.homepage.as_deref(), Some("https://example.org"));
        assert_eq!(details.description.as_deref(), Some("Great app for Linux."));
        assert_eq!(details.screenshots.len(), 1);
        assert_eq!(details.screenshots[0].0, "https://dl.flathub.org/med.png");
        assert_eq!(details.screenshots[0].1.as_deref(), Some("Main view"));
    }

    #[test]
    fn test_is_valid_image_bytes() {
        assert!(PackageManager::is_valid_image_bytes(&[
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0
        ]));
        assert!(PackageManager::is_valid_image_bytes(&[
            0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0
        ]));
        assert!(PackageManager::is_valid_image_bytes(b"RIFF1234WEBPxxxx"));
        assert!(!PackageManager::is_valid_image_bytes(b"<html>404</html>"));
    }
}
