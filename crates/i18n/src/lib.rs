//! # Corten Internationalization (`corten-i18n`)
//!
//! Strongly-typed, zero-runtime-overhead translation dictionary for Corten Desktop.
//! Supports: Türkçe (tr), English (en), Deutsch (de), Français (fr), Español (es), Русский (ru), 日本語 (ja).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Tr,
    En,
    De,
    Fr,
    Es,
    Ru,
    Ja,
}

impl Language {
    pub const ALL: [Language; 7] = [
        Language::Tr,
        Language::En,
        Language::De,
        Language::Fr,
        Language::Es,
        Language::Ru,
        Language::Ja,
    ];

    pub fn code(&self) -> &'static str {
        match self {
            Language::Tr => "tr",
            Language::En => "en",
            Language::De => "de",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::Ru => "ru",
            Language::Ja => "ja",
        }
    }

    pub fn native_name(&self) -> &'static str {
        match self {
            Language::Tr => "Türkçe",
            Language::En => "English",
            Language::De => "Deutsch",
            Language::Fr => "Français",
            Language::Es => "Español",
            Language::Ru => "Русский",
            Language::Ja => "日本語",
        }
    }

    pub fn flag(&self) -> &'static str {
        match self {
            Language::Tr => "🇹🇷",
            Language::En => "🇬🇧",
            Language::De => "🇩🇪",
            Language::Fr => "🇫🇷",
            Language::Es => "🇪🇸",
            Language::Ru => "🇷🇺",
            Language::Ja => "🇯🇵",
        }
    }

    pub fn from_code(code: &str) -> Self {
        let clean = code.split('.').next().unwrap_or(code).to_lowercase();
        if clean.starts_with("tr") {
            Language::Tr
        } else if clean.starts_with("de") {
            Language::De
        } else if clean.starts_with("fr") {
            Language::Fr
        } else if clean.starts_with("es") {
            Language::Es
        } else if clean.starts_with("ru") {
            Language::Ru
        } else if clean.starts_with("ja") {
            Language::Ja
        } else {
            Language::En
        }
    }

    pub fn detect_system() -> Self {
        if let Ok(lang) = std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")) {
            Self::from_code(&lang)
        } else {
            Language::Tr
        }
    }

    pub fn t(&self) -> &'static Strings {
        match self {
            Language::Tr => &TR,
            Language::En => &EN,
            Language::De => &DE,
            Language::Fr => &FR,
            Language::Es => &ES,
            Language::Ru => &RU,
            Language::Ja => &JA,
        }
    }

    pub fn format_date(&self, weekday_1_to_7: u32, day: u32, month_1_to_12: u32) -> String {
        match self {
            Language::Tr => {
                let w = match weekday_1_to_7 {
                    1 => "Pazartesi", 2 => "Salı", 3 => "Çarşamba", 4 => "Perşembe",
                    5 => "Cuma", 6 => "Cumartesi", _ => "Pazar",
                };
                let m = match month_1_to_12 {
                    1 => "Ocak", 2 => "Şubat", 3 => "Mart", 4 => "Nisan", 5 => "Mayıs", 6 => "Haziran",
                    7 => "Temmuz", 8 => "Ağustos", 9 => "Eylül", 10 => "Ekim", 11 => "Kasım", _ => "Aralık",
                };
                format!("{}, {} {}", w, day, m)
            }
            Language::En => {
                let w = match weekday_1_to_7 {
                    1 => "Monday", 2 => "Tuesday", 3 => "Wednesday", 4 => "Thursday",
                    5 => "Friday", 6 => "Saturday", _ => "Sunday",
                };
                let m = match month_1_to_12 {
                    1 => "January", 2 => "February", 3 => "March", 4 => "April", 5 => "May", 6 => "June",
                    7 => "July", 8 => "August", 9 => "September", 10 => "October", 11 => "November", _ => "December",
                };
                format!("{}, {} {}", w, m, day)
            }
            Language::De => {
                let w = match weekday_1_to_7 {
                    1 => "Montag", 2 => "Dienstag", 3 => "Mittwoch", 4 => "Donnerstag",
                    5 => "Freitag", 6 => "Samstag", _ => "Sonntag",
                };
                let m = match month_1_to_12 {
                    1 => "Januar", 2 => "Februar", 3 => "März", 4 => "April", 5 => "Mai", 6 => "Juni",
                    7 => "Juli", 8 => "August", 9 => "September", 10 => "Oktober", 11 => "November", _ => "Dezember",
                };
                format!("{}, {}. {}", w, day, m)
            }
            Language::Fr => {
                let w = match weekday_1_to_7 {
                    1 => "Lundi", 2 => "Mardi", 3 => "Mercredi", 4 => "Jeudi",
                    5 => "Vendredi", 6 => "Samedi", _ => "Dimanche",
                };
                let m = match month_1_to_12 {
                    1 => "janvier", 2 => "février", 3 => "mars", 4 => "avril", 5 => "mai", 6 => "juin",
                    7 => "juillet", 8 => "août", 9 => "septembre", 10 => "octobre", 11 => "novembre", _ => "décembre",
                };
                format!("{} {} {}", w, day, m)
            }
            Language::Es => {
                let w = match weekday_1_to_7 {
                    1 => "Lunes", 2 => "Martes", 3 => "Miércoles", 4 => "Jueves",
                    5 => "Viernes", 6 => "Sábado", _ => "Domingo",
                };
                let m = match month_1_to_12 {
                    1 => "enero", 2 => "febrero", 3 => "marzo", 4 => "abril", 5 => "mayo", 6 => "junio",
                    7 => "julio", 8 => "agosto", 9 => "septiembre", 10 => "octubre", 11 => "noviembre", _ => "diciembre",
                };
                format!("{}, {} de {}", w, day, m)
            }
            Language::Ru => {
                let w = match weekday_1_to_7 {
                    1 => "Понедельник", 2 => "Вторник", 3 => "Среда", 4 => "Четверг",
                    5 => "Пятница", 6 => "Суббота", _ => "Воскресенье",
                };
                let m = match month_1_to_12 {
                    1 => "января", 2 => "февраля", 3 => "марта", 4 => "апреля", 5 => "мая", 6 => "июня",
                    7 => "июля", 8 => "августа", 9 => "сентября", 10 => "октября", 11 => "ноября", _ => "декабря",
                };
                format!("{}, {} {}", w, day, m)
            }
            Language::Ja => {
                let w = match weekday_1_to_7 {
                    1 => "月", 2 => "火", 3 => "水", 4 => "木",
                    5 => "金", 6 => "土", _ => "日",
                };
                format!("{}月{}日 ({})", month_1_to_12, day, w)
            }
        }
    }
}

pub struct Strings {
    // Brand & General
    pub brand_title: &'static str,
    pub brand_subtitle: &'static str,

    // Welcome Wizard
    pub step_intro: &'static str,
    pub step_appearance: &'static str,
    pub step_shortcuts: &'static str,
    pub step_ai_privacy: &'static str,
    pub step_finish: &'static str,
    pub welcome_headline: &'static str,
    pub welcome_subline: &'static str,
    pub appearance_headline: &'static str,
    pub appearance_subline: &'static str,
    pub shortcuts_headline: &'static str,
    pub shortcuts_subline: &'static str,
    pub ai_privacy_headline: &'static str,
    pub ai_privacy_subline: &'static str,
    pub finish_headline: &'static str,
    pub finish_subline: &'static str,
    pub theme_dark: &'static str,
    pub theme_dark_desc: &'static str,
    pub theme_light: &'static str,
    pub theme_light_desc: &'static str,
    pub wallpaper_title: &'static str,
    pub wallpaper_patina: &'static str,
    pub wallpaper_patina_desc: &'static str,
    pub wallpaper_forged: &'static str,
    pub wallpaper_forged_desc: &'static str,
    pub wallpaper_keep: &'static str,
    pub wallpaper_keep_desc: &'static str,
    pub night_light_label: &'static str,
    pub btn_back: &'static str,
    pub btn_next: &'static str,
    pub btn_explore: &'static str,
    pub dont_show_again: &'static str,
    pub reopen_info: &'static str,

    // Power & Session Hub
    pub power_hub_title: &'static str,
    pub power_lock: &'static str,
    pub power_lock_desc: &'static str,
    pub power_suspend: &'static str,
    pub power_suspend_desc: &'static str,
    pub power_logout: &'static str,
    pub power_logout_desc: &'static str,
    pub power_reboot: &'static str,
    pub power_reboot_desc: &'static str,
    pub power_poweroff: &'static str,
    pub power_poweroff_desc: &'static str,
    pub power_confirm_reboot: &'static str,
    pub power_confirm_reboot_desc: &'static str,
    pub power_confirm_poweroff: &'static str,
    pub power_confirm_poweroff_desc: &'static str,
    pub btn_cancel: &'static str,
    pub btn_confirm: &'static str,

    // Weather & Calendar
    pub weather_calendar_title: &'static str,
    pub weather_humidity: &'static str,
    pub weather_wind: &'static str,
    pub weather_tomorrow: &'static str,

    // Shortcuts Cheat Sheet
    pub cheat_sheet_title: &'static str,
    pub cheat_sheet_subtitle: &'static str,
    pub cat_windows: &'static str,
    pub cat_system: &'static str,
    pub cat_apps: &'static str,
    pub shortcut_launcher: &'static str,
    pub shortcut_overview: &'static str,
    pub shortcut_terminal: &'static str,
    pub shortcut_files: &'static str,
    pub shortcut_notes: &'static str,
    pub shortcut_close: &'static str,
    pub shortcut_fullscreen: &'static str,
    pub shortcut_power: &'static str,
    pub shortcut_settings: &'static str,

    // Workspaces
    pub ws_code: &'static str,
    pub ws_web: &'static str,
    pub ws_chat: &'static str,
    pub ws_media: &'static str,

    // Control Center
    pub control_center_title: &'static str,
    pub audio_output_device: &'static str,
    pub wifi_available_networks: &'static str,
}

// ---------------- Türkçe (tr) ----------------
const TR: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Wayland Masaüstü",

    step_intro: "1. Giriş & Sistem",
    step_appearance: "2. Görünüm & Tema",
    step_shortcuts: "3. Hızlı Kısayollar",
    step_ai_privacy: "4. Yapay Zekâ & Gizlilik",
    step_finish: "5. Keşfetmeye Başla",

    welcome_headline: "Corten Desktop'a Hoş Geldiniz",
    welcome_subline: "Rust ve Smithay tabanlı modern, rijit ve akıllı Wayland masaüstü deneyimi.",

    appearance_headline: "Görünüm & Atmosfer",
    appearance_subline: "Masaüstü temanızı, duvar kâğıdınızı ve gece ışığı tercihlerinizi belirleyin.",

    shortcuts_headline: "Klavye Odaklı İş Akışı",
    shortcuts_subline: "Farenize dokunmadan tüm sistemi yönetecek temel Corten kısayolları.",

    ai_privacy_headline: "Yapay Zekâ & Donanım Gizliliği",
    ai_privacy_subline: "Spotlight doğal dil komutları ve sıfır gecikmeli ALSA donanım izleme merkezi.",

    finish_headline: "Her Şey Hazır!",
    finish_subline: "Tercihleriniz uygulandı. Corten masaüstünü keşfetmeye başlayabilirsiniz.",

    theme_dark: "Koyu Mod (Dark Corten)",
    theme_dark_desc: "Antrasit zemin, sıcak Cor-Ten pas ve bakır vurguları.",
    theme_light: "Açık Mod (Brushed Steel)",
    theme_light_desc: "Fırçalanmış paslanmaz çelik ve açık gri çalışma alanı.",

    wallpaper_title: "Duvar Kâğıdı Seçimi",
    wallpaper_patina: "Corten Dark Patina (Yatay)",
    wallpaper_patina_desc: "Derin pas dokulu, minimal ve endüstriyel soyut zemin",
    wallpaper_forged: "Corten Forged Rust (Dikey / Dual)",
    wallpaper_forged_desc: "Dövme çelik plakalar ve sıcak kehribar parlamalar",
    wallpaper_keep: "Varsayılan / Değiştirme",
    wallpaper_keep_desc: "Mevcut sistem veya seçili duvar kâğıdını koru",

    night_light_label: "🌙 Akşam saatlerinde Gece Işığını otomatik etkinleştir (Ekran Sıcaklığı)",
    btn_back: "Geri",
    btn_next: "İleri →",
    btn_explore: "🚀 Masaüstünü Keşfet",
    dont_show_again: "Bu hoş geldin sihirbazını bir sonraki açılışta gösterme",
    reopen_info: "İleride dilediğiniz zaman Ayarlar menüsünden veya 'corten-welcome' komutuyla bu ekrana dönebilirsiniz.",

    power_hub_title: "⚡ Güç & Oturum Merkezi",
    power_lock: "Ekranı Kilitle",
    power_lock_desc: "Oturumu kilitler ve kimlik doğrulama ister",
    power_suspend: "Askıya Al (Uyku)",
    power_suspend_desc: "Düşük güç tüketimli bekleme moduna geçer",
    power_logout: "Oturumu Kapat",
    power_logout_desc: "Tüm açık süreçleri kapatır ve greeter'a döner",
    power_reboot: "Yeniden Başlat",
    power_reboot_desc: "İşletim sistemini güvenle yeniden başlatır",
    power_poweroff: "Sistemi Kapat",
    power_poweroff_desc: "Bilgisayarın gücünü tamamen kapatır",
    power_confirm_reboot: "Sistem Yeniden Başlatılsın mı?",
    power_confirm_reboot_desc: "Tüm açık pencereler kapatılacak ve sistem yeniden başlayacaktır.",
    power_confirm_poweroff: "Sistem Kapatılsın mı?",
    power_confirm_poweroff_desc: "Kaydedilmemiş tüm veriler kaybolabilir. Güç tamamen kesilecektir.",
    btn_cancel: "Vazgeç / İptal",
    btn_confirm: "Evet, Onayla",

    weather_calendar_title: "📅 Takvim & Hava Durumu",
    weather_humidity: "Nem",
    weather_wind: "Rüzgar",
    weather_tomorrow: "Yarın",

    cheat_sheet_title: "⌨️ Corten Klavye Kısayolları Rehberi",
    cheat_sheet_subtitle: "Tüm sistemi farenize dokunmadan hızla yönetin. Kapatmak için Esc'ye veya herhangi bir tuşa basın.",
    cat_windows: "PENCERE & ÇALIŞMA ALANLARI",
    cat_system: "SİSTEM & GÜÇ",
    cat_apps: "UYGULAMALAR",
    shortcut_launcher: "Spotlight / Uygulama ve Komut Başlatıcı",
    shortcut_overview: "Pencere Genel Bakışı (Exposé)",
    shortcut_terminal: "Alacritty Terminal",
    shortcut_files: "Corten Dosya Yöneticisi",
    shortcut_notes: "Yüzen Karalama Defteri (Notes)",
    shortcut_close: "Aktif Pencereyi Kapat",
    shortcut_fullscreen: "Tam Ekran / Büyüt",
    shortcut_power: "Güç & Oturum Merkezi",
    shortcut_settings: "Corten Sistem Ayarları",

    ws_code: "💻 Kod",
    ws_web: "🌐 Web",
    ws_chat: "💬 Sohbet",
    ws_media: "🎵 Medya",

    control_center_title: "Denetim Merkezi",
    audio_output_device: "Ses Çıkış Aygıtı",
    wifi_available_networks: "Kullanılabilir Wi-Fi Ağları",
};

// ---------------- English (en) ----------------
const EN: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Wayland Desktop",

    step_intro: "1. Intro & System",
    step_appearance: "2. Appearance & Theme",
    step_shortcuts: "3. Quick Shortcuts",
    step_ai_privacy: "4. AI & Privacy",
    step_finish: "5. Start Exploring",

    welcome_headline: "Welcome to Corten Desktop",
    welcome_subline: "A modern, rigid, and intelligent Wayland desktop environment forged in pure Rust.",

    appearance_headline: "Appearance & Atmosphere",
    appearance_subline: "Configure your desktop theme, wallpaper, and night light preferences.",

    shortcuts_headline: "Keyboard-Driven Workflow",
    shortcuts_subline: "Essential Corten shortcuts to control your entire system without touching a mouse.",

    ai_privacy_headline: "AI & Hardware Privacy",
    ai_privacy_subline: "Natural language Spotlight actions and zero-latency kernel ALSA hardware monitoring.",

    finish_headline: "All Set!",
    finish_subline: "Your preferences have been applied. You are ready to explore Corten Desktop.",

    theme_dark: "Dark Mode (Dark Corten)",
    theme_dark_desc: "Anthracite background with warm Cor-Ten rust and copper accents.",
    theme_light: "Light Mode (Brushed Steel)",
    theme_light_desc: "Brushed stainless steel tones and a clean, high-contrast workspace.",

    wallpaper_title: "Wallpaper Selection",
    wallpaper_patina: "Corten Dark Patina (Landscape)",
    wallpaper_patina_desc: "Deep weathered patina texture, minimal and industrial abstract backdrop",
    wallpaper_forged: "Corten Forged Rust (Portrait / Dual)",
    wallpaper_forged_desc: "Heavy forged steel plates with warm radiant amber glow",
    wallpaper_keep: "Default / Keep Current",
    wallpaper_keep_desc: "Retain current system wallpaper or background setting",

    night_light_label: "🌙 Automatically activate Night Light during evening hours (Color Warmth)",
    btn_back: "Back",
    btn_next: "Next →",
    btn_explore: "🚀 Explore Desktop",
    dont_show_again: "Do not show this welcome wizard on next boot",
    reopen_info: "You can reopen this wizard anytime from the App Menu or by running 'corten-welcome'.",

    power_hub_title: "⚡ Power & Session Hub",
    power_lock: "Lock Screen",
    power_lock_desc: "Locks the session and requests authentication",
    power_suspend: "Suspend (Sleep)",
    power_suspend_desc: "Enters low-power standby mode",
    power_logout: "Log Out",
    power_logout_desc: "Closes all running processes and returns to greeter",
    power_reboot: "Reboot",
    power_reboot_desc: "Safely restarts the operating system",
    power_poweroff: "Power Off",
    power_poweroff_desc: "Shuts down system power completely",
    power_confirm_reboot: "Restart the system?",
    power_confirm_reboot_desc: "All open applications will close and the system will reboot.",
    power_confirm_poweroff: "Shut down the system?",
    power_confirm_poweroff_desc: "Unsaved work will be lost. Power will be completely cut.",
    btn_cancel: "Cancel",
    btn_confirm: "Yes, Confirm",

    weather_calendar_title: "📅 Calendar & Weather",
    weather_humidity: "Humidity",
    weather_wind: "Wind",
    weather_tomorrow: "Tomorrow",

    cheat_sheet_title: "⌨️ Corten Keyboard Shortcuts Guide",
    cheat_sheet_subtitle: "Navigate the entire desktop without leaving the keyboard. Press Esc or any key to close.",
    cat_windows: "WINDOWS & WORKSPACES",
    cat_system: "SYSTEM & POWER",
    cat_apps: "APPLICATIONS",
    shortcut_launcher: "Spotlight / App & Command Launcher",
    shortcut_overview: "Window Overview (Exposé)",
    shortcut_terminal: "Alacritty Terminal",
    shortcut_files: "Corten File Manager",
    shortcut_notes: "Floating Scratchpad Notes",
    shortcut_close: "Close Active Window",
    shortcut_fullscreen: "Toggle Fullscreen",
    shortcut_power: "Power & Session Hub",
    shortcut_settings: "Corten System Settings",

    ws_code: "💻 Code",
    ws_web: "🌐 Web",
    ws_chat: "💬 Chat",
    ws_media: "🎵 Media",

    control_center_title: "Control Center",
    audio_output_device: "Audio Output Device",
    wifi_available_networks: "Available Wi-Fi Networks",
};

// ---------------- Deutsch (de) ----------------
const DE: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Wayland Desktop",

    step_intro: "1. Intro & System",
    step_appearance: "2. Design & Thema",
    step_shortcuts: "3. Tastenkürzel",
    step_ai_privacy: "4. KI & Datenschutz",
    step_finish: "5. Erkunden",

    welcome_headline: "Willkommen bei Corten Desktop",
    welcome_subline: "Eine moderne, robuste und intelligente Wayland-Umgebung, geschmiedet in reinem Rust.",

    appearance_headline: "Erscheinungsbild & Atmosphäre",
    appearance_subline: "Konfigurieren Sie Design, Hintergrundbild und Nachtmodus.",

    shortcuts_headline: "Tastaturgesteuerter Workflow",
    shortcuts_subline: "Wichtige Tastenkürzel zur Steuerung des gesamten Systems ohne Maus.",

    ai_privacy_headline: "KI & Hardware-Datenschutz",
    ai_privacy_subline: "Spotlight-Befehle in natürlicher Sprache und latenzfreie ALSA-Hardwareüberwachung.",

    finish_headline: "Alles bereit!",
    finish_subline: "Ihre Einstellungen wurden übernommen. Entdecken Sie Corten Desktop.",

    theme_dark: "Dunkelmodus (Dark Corten)",
    theme_dark_desc: "Anthrazitfarbener Hintergrund mit warmen Cor-Ten-Rost- und Kupferakzenten.",
    theme_light: "Hellmodus (Brushed Steel)",
    theme_light_desc: "Gebürsteter Edelstahl und klarer, kontrastreicher Arbeitsbereich.",

    wallpaper_title: "Hintergrundbild auswählen",
    wallpaper_patina: "Corten Dark Patina (Querformat)",
    wallpaper_patina_desc: "Tief verwitterte Patina-Textur, minimalistisch und industriell",
    wallpaper_forged: "Corten Forged Rust (Hochformat / Dual)",
    wallpaper_forged_desc: "Schwere geschmiedete Stahlplatten mit warmem bernsteinfarbenem Glanz",
    wallpaper_keep: "Aktuelles behalten",
    wallpaper_keep_desc: "Bestehendes Hintergrundbild beibehalten",

    night_light_label: "🌙 Nachtmodus abends automatisch aktivieren (Farbtemperatur)",
    btn_back: "Zurück",
    btn_next: "Weiter →",
    btn_explore: "🚀 Desktop erkunden",
    dont_show_again: "Diesen Assistenten beim nächsten Start nicht mehr anzeigen",
    reopen_info: "Sie können diesen Assistenten jederzeit über das Menü oder mit 'corten-welcome' öffnen.",

    power_hub_title: "⚡ Energie & Sitzungszentrale",
    power_lock: "Bildschirm sperren",
    power_lock_desc: "Sperrt die Sitzung und fordert Authentifizierung",
    power_suspend: "Bereitschaft (Schlafen)",
    power_suspend_desc: "Wechselt in den Energiesparmodus",
    power_logout: "Abmelden",
    power_logout_desc: "Schließt alle Prozesse und kehrt zum Anmeldebildschirm zurück",
    power_reboot: "Neu starten",
    power_reboot_desc: "Startet das System sicher neu",
    power_poweroff: "Herunterfahren",
    power_poweroff_desc: "Schaltet den Computer vollständig aus",
    power_confirm_reboot: "System neu starten?",
    power_confirm_reboot_desc: "Alle offenen Anwendungen werden geschlossen.",
    power_confirm_poweroff: "System herunterfahren?",
    power_confirm_poweroff_desc: "Ungespeicherte Änderungen gehen verloren.",
    btn_cancel: "Abbrechen",
    btn_confirm: "Ja, Bestätigen",

    weather_calendar_title: "📅 Kalender & Wetter",
    weather_humidity: "Luftfeuchtigkeit",
    weather_wind: "Wind",
    weather_tomorrow: "Morgen",

    cheat_sheet_title: "⌨️ Corten Tastenkürzel-Übersicht",
    cheat_sheet_subtitle: "Bedienen Sie das gesamte System über die Tastatur. Drücken Sie Esc zum Schließen.",
    cat_windows: "FENSTER & ARBEITSFLÄCHEN",
    cat_system: "SYSTEM & ENERGIE",
    cat_apps: "ANWENDUNGEN",
    shortcut_launcher: "Spotlight / App-Starter",
    shortcut_overview: "Fensterübersicht (Exposé)",
    shortcut_terminal: "Alacritty Terminal",
    shortcut_files: "Corten Dateimanager",
    shortcut_notes: "Notizblock (Scratchpad)",
    shortcut_close: "Aktives Fenster schließen",
    shortcut_fullscreen: "Vollbild umschalten",
    shortcut_power: "Energiezentrale",
    shortcut_settings: "Systemeinstellungen",

    ws_code: "💻 Code",
    ws_web: "🌐 Web",
    ws_chat: "💬 Chat",
    ws_media: "🎵 Medien",

    control_center_title: "Kontrollzentrum",
    audio_output_device: "Audio-Ausgabegerät",
    wifi_available_networks: "Verfügbare WLAN-Netzwerke",
};

// ---------------- Français (fr) ----------------
const FR: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Bureau Wayland",

    step_intro: "1. Intro & Système",
    step_appearance: "2. Apparence & Thème",
    step_shortcuts: "3. Raccourcis Clavier",
    step_ai_privacy: "4. IA & Confidentialité",
    step_finish: "5. Explorer",

    welcome_headline: "Bienvenue sur Corten Desktop",
    welcome_subline: "Un environnement de bureau Wayland moderne, robuste et intelligent forgé en pur Rust.",

    appearance_headline: "Apparence & Ambiance",
    appearance_subline: "Configurez votre thème, fond d'écran et mode éclairage nocturne.",

    shortcuts_headline: "Flux de Travail au Clavier",
    shortcuts_subline: "Raccourcis essentiels pour contrôler l'ensemble du système sans toucher la souris.",

    ai_privacy_headline: "IA & Confidentialité Matérielle",
    ai_privacy_subline: "Commandes naturelles Spotlight et surveillance matérielle ALSA à latence zéro.",

    finish_headline: "Tout est Prêt !",
    finish_subline: "Vos préférences sont appliquées. Explorez l'expérience Corten.",

    theme_dark: "Mode Sombre (Dark Corten)",
    theme_dark_desc: "Fond anthracite avec accents chaleureux de rouille Cor-Ten et cuivre.",
    theme_light: "Mode Clair (Brushed Steel)",
    theme_light_desc: "Tons acier inoxydable brossé et espace de travail net et contrasté.",

    wallpaper_title: "Sélection du Fond d'Écran",
    wallpaper_patina: "Corten Dark Patina (Paysage)",
    wallpaper_patina_desc: "Texture de patine industrielle et abstraite",
    wallpaper_forged: "Corten Forged Rust (Portrait / Double)",
    wallpaper_forged_desc: "Plaques d'acier lourd avec éclat ambré chaleureux",
    wallpaper_keep: "Conserver l'Actuel",
    wallpaper_keep_desc: "Garder le fond d'écran déjà configuré",

    night_light_label: "🌙 Activer automatiquement l'éclairage nocturne le soir",
    btn_back: "Retour",
    btn_next: "Suivant →",
    btn_explore: "🚀 Explorer le Bureau",
    dont_show_again: "Ne plus afficher cet assistant au prochain démarrage",
    reopen_info: "Vous pouvez rouvrir cet assistant depuis le menu ou avec 'corten-welcome'.",

    power_hub_title: "⚡ Centre d'Alimentation & Session",
    power_lock: "Verrouiller l'écran",
    power_lock_desc: "Verrouille la session et demande une authentification",
    power_suspend: "Mettre en veille",
    power_suspend_desc: "Passe en mode basse consommation",
    power_logout: "Se déconnecter",
    power_logout_desc: "Ferme les processus et retourne à l'écran de connexion",
    power_reboot: "Redémarrer",
    power_reboot_desc: "Redémarre l'ordinateur en toute sécurité",
    power_poweroff: "Éteindre",
    power_poweroff_desc: "Arrête complètement l'alimentation",
    power_confirm_reboot: "Redémarrer le système ?",
    power_confirm_reboot_desc: "Toutes les fenêtres ouvertes seront fermées.",
    power_confirm_poweroff: "Éteindre le système ?",
    power_confirm_poweroff_desc: "Les travaux non enregistrés seront perdus.",
    btn_cancel: "Annuler",
    btn_confirm: "Oui, Confirmer",

    weather_calendar_title: "📅 Calendrier & Météo",
    weather_humidity: "Humidité",
    weather_wind: "Vent",
    weather_tomorrow: "Demain",

    cheat_sheet_title: "⌨️ Guide des Raccourcis Clavier Corten",
    cheat_sheet_subtitle: "Contrôlez l'ensemble du bureau sans souris. Appuyez sur Échap pour fermer.",
    cat_windows: "FENÊTRES & ESPACES DE TRAVAIL",
    cat_system: "SYSTÈME & ALIMENTATION",
    cat_apps: "APPLICATIONS",
    shortcut_launcher: "Spotlight / Lanceur d'applications",
    shortcut_overview: "Aperçu des fenêtres (Exposé)",
    shortcut_terminal: "Terminal Alacritty",
    shortcut_files: "Gestionnaire de fichiers Corten",
    shortcut_notes: "Bloc-notes flottant",
    shortcut_close: "Fermer la fenêtre active",
    shortcut_fullscreen: "Basculer en plein écran",
    shortcut_power: "Centre d'alimentation",
    shortcut_settings: "Paramètres Corten",

    ws_code: "💻 Code",
    ws_web: "🌐 Web",
    ws_chat: "💬 Chat",
    ws_media: "🎵 Média",

    control_center_title: "Centre de contrôle",
    audio_output_device: "Périphérique de sortie audio",
    wifi_available_networks: "Réseaux Wi-Fi disponibles",
};

// ---------------- Español (es) ----------------
const ES: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Escritorio Wayland",

    step_intro: "1. Intro & Sistema",
    step_appearance: "2. Apariencia & Tema",
    step_shortcuts: "3. Atajos de Teclado",
    step_ai_privacy: "4. IA & Privacidad",
    step_finish: "5. Explorar",

    welcome_headline: "Bienvenido a Corten Desktop",
    welcome_subline: "Un entorno Wayland moderno, sólido e inteligente forjado en Rust puro.",

    appearance_headline: "Apariencia & Entorno",
    appearance_subline: "Configure su tema, fondo de pantalla y luz nocturna.",

    shortcuts_headline: "Flujo de Trabajo con Teclado",
    shortcuts_subline: "Atajos clave de Corten para controlar el sistema sin tocar el ratón.",

    ai_privacy_headline: "IA & Privacidad de Hardware",
    ai_privacy_subline: "Comandos naturales con Spotlight y monitoreo ALSA en tiempo real.",

    finish_headline: "¡Todo Listo!",
    finish_subline: "Sus preferencias han sido aplicadas. Disfrute de Corten Desktop.",

    theme_dark: "Modo Oscuro (Dark Corten)",
    theme_dark_desc: "Fondo antracita con detalles cálidos de óxido Cor-Ten y cobre.",
    theme_light: "Modo Claro (Brushed Steel)",
    theme_light_desc: "Tonos de acero inoxidable cepillado con alto contraste.",

    wallpaper_title: "Selección de Fondo de Pantalla",
    wallpaper_patina: "Corten Dark Patina (Horizontal)",
    wallpaper_patina_desc: "Textura de pátina oxidada, abstracta e industrial",
    wallpaper_forged: "Corten Forged Rust (Vertical / Dual)",
    wallpaper_forged_desc: "Placas de acero forjado con brillo ámbar radiante",
    wallpaper_keep: "Conservar Actual",
    wallpaper_keep_desc: "Mantener el fondo de pantalla configurado actualmente",

    night_light_label: "🌙 Activar automáticamente la luz nocturna por la tarde",
    btn_back: "Atrás",
    btn_next: "Siguiente →",
    btn_explore: "🚀 Explorar Escritorio",
    dont_show_again: "No volver a mostrar este asistente en el próximo inicio",
    reopen_info: "Puede volver a abrir este asistente desde el menú o con 'corten-welcome'.",

    power_hub_title: "⚡ Centro de Energía & Sesión",
    power_lock: "Bloquear Pantalla",
    power_lock_desc: "Bloquea la sesión y solicita autenticación",
    power_suspend: "Suspender (Dormir)",
    power_suspend_desc: "Entra en modo de bajo consumo",
    power_logout: "Cerrar Sesión",
    power_logout_desc: "Cierra procesos y vuelve a la pantalla de bienvenida",
    power_reboot: "Reiniciar",
    power_reboot_desc: "Reinicia el sistema de forma segura",
    power_poweroff: "Apagar",
    power_poweroff_desc: "Apaga por completo la energía del equipo",
    power_confirm_reboot: "¿Reiniciar el sistema?",
    power_confirm_reboot_desc: "Todas las ventanas abiertas se cerrarán.",
    power_confirm_poweroff: "¿Apagar el sistema?",
    power_confirm_poweroff_desc: "Los cambios no guardados se perderán.",
    btn_cancel: "Cancelar",
    btn_confirm: "Sí, Confirmar",

    weather_calendar_title: "📅 Calendario & Clima",
    weather_humidity: "Humedad",
    weather_wind: "Viento",
    weather_tomorrow: "Mañana",

    cheat_sheet_title: "⌨️ Guía de Atajos de Teclado Corten",
    cheat_sheet_subtitle: "Gestione todo el escritorio desde el teclado. Pulse Esc para cerrar.",
    cat_windows: "VENTANAS & ESPACIOS DE TRABAJO",
    cat_system: "SISTEMA & ENERGÍA",
    cat_apps: "APLICACIONES",
    shortcut_launcher: "Spotlight / Lanzador de aplicaciones",
    shortcut_overview: "Vista general de ventanas (Exposé)",
    shortcut_terminal: "Terminal Alacritty",
    shortcut_files: "Explorador de archivos Corten",
    shortcut_notes: "Bloc de notas flotante",
    shortcut_close: "Cerrar ventana activa",
    shortcut_fullscreen: "Pantalla completa",
    shortcut_power: "Centro de energía",
    shortcut_settings: "Ajustes del sistema",

    ws_code: "💻 Código",
    ws_web: "🌐 Web",
    ws_chat: "💬 Chat",
    ws_media: "🎵 Medios",

    control_center_title: "Centro de control",
    audio_output_device: "Dispositivo de salida de audio",
    wifi_available_networks: "Redes Wi-Fi disponibles",
};

// ---------------- Русский (ru) ----------------
const RU: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Wayland Окружение",

    step_intro: "1. Система",
    step_appearance: "2. Оформление",
    step_shortcuts: "3. Горячие Клавиши",
    step_ai_privacy: "4. ИИ и Приватность",
    step_finish: "5. Завершение",

    welcome_headline: "Добро пожаловать в Corten Desktop",
    welcome_subline: "Современное, надежное и быстрое Wayland окружение, созданное на чистом Rust.",

    appearance_headline: "Оформление и Атмосфера",
    appearance_subline: "Настройте тему оформления, обои и ночной режим экрана.",

    shortcuts_headline: "Управление с Клавиатуры",
    shortcuts_subline: "Основные горячие клавиши для полного контроля над системой без мыши.",

    ai_privacy_headline: "ИИ и Аппаратная Безопасность",
    ai_privacy_subline: "Команды на естественном языке в Spotlight и прямой мониторинг микрофона в ALSA.",

    finish_headline: "Всё Готово!",
    finish_subline: "Ваши настройки успешно применены. Начните работу в Corten Desktop.",

    theme_dark: "Тёмная Тема (Dark Corten)",
    theme_dark_desc: "Антрацитовый фон с теплыми медными и стальными акцентами.",
    theme_light: "Светлая Тема (Brushed Steel)",
    theme_light_desc: "Текстура шлифованной нержавеющей стали и высокая контрастность.",

    wallpaper_title: "Выбор Обоев",
    wallpaper_patina: "Corten Dark Patina (Горизонтальные)",
    wallpaper_patina_desc: "Глубокая текстура патины и индустриальный минимализм",
    wallpaper_forged: "Corten Forged Rust (Вертикальные / Dual)",
    wallpaper_forged_desc: "Кованые стальные плиты с теплым янтарным свечением",
    wallpaper_keep: "Оставить Текущие",
    wallpaper_keep_desc: "Сохранить текущие обои системы",

    night_light_label: "🌙 Автоматически включать ночной свет в вечернее время",
    btn_back: "Назад",
    btn_next: "Далее →",
    btn_explore: "🚀 Начать Работу",
    dont_show_again: "Больше не показывать этот мастер при входе",
    reopen_info: "Вы можете открыть этот мастер в любое время из меню или командой 'corten-welcome'.",

    power_hub_title: "⚡ Центр Питания и Сеанса",
    power_lock: "Заблокировать экран",
    power_lock_desc: "Блокирует сеанс и запрашивает пароль",
    power_suspend: "Спящий режим",
    power_suspend_desc: "Переход в режим низкого энергопотребления",
    power_logout: "Завершить сеанс",
    power_logout_desc: "Закрывает программы и возвращает на экран входа",
    power_reboot: "Перезагрузка",
    power_reboot_desc: "Безопасный перезапуск операционной системы",
    power_poweroff: "Выключение",
    power_poweroff_desc: "Полное отключение питания компьютера",
    power_confirm_reboot: "Перезагрузить компьютер?",
    power_confirm_reboot_desc: "Все открытые программы будут закрыты.",
    power_confirm_poweroff: "Выключить компьютер?",
    power_confirm_poweroff_desc: "Несохраненные данные будут потеряны.",
    btn_cancel: "Отмена",
    btn_confirm: "Да, Подтвердить",

    weather_calendar_title: "📅 Календарь и Погода",
    weather_humidity: "Влажность",
    weather_wind: "Ветер",
    weather_tomorrow: "Завтра",

    cheat_sheet_title: "⌨️ Справочник Горячих Клавиш Corten",
    cheat_sheet_subtitle: "Управляйте всей системой с клавиатуры. Нажмите Esc для закрытия.",
    cat_windows: "ОКНА И РАБОЧИЕ СТОЛЫ",
    cat_system: "СИСТЕМА И ПИТАНИЕ",
    cat_apps: "ПРИЛОЖЕНИЯ",
    shortcut_launcher: "Spotlight / Запуск программ и команд",
    shortcut_overview: "Обзор окон (Exposé)",
    shortcut_terminal: "Терминал Alacritty",
    shortcut_files: "Файловый менеджер Corten",
    shortcut_notes: "Плавающий блокнот для заметок",
    shortcut_close: "Закрыть активное окно",
    shortcut_fullscreen: "Полноэкранный режим",
    shortcut_power: "Центр питания",
    shortcut_settings: "Настройки Corten",

    ws_code: "💻 Код",
    ws_web: "🌐 Сеть",
    ws_chat: "💬 Чат",
    ws_media: "🎵 Медиа",

    control_center_title: "Центр управления",
    audio_output_device: "Устройство вывода звука",
    wifi_available_networks: "Доступные сети Wi-Fi",
};

// ---------------- 日本語 (ja) ----------------
const JA: Strings = Strings {
    brand_title: "CORTEN",
    brand_subtitle: "Wayland デスクトップ",

    step_intro: "1. システム概要",
    step_appearance: "2. 外観とテーマ",
    step_shortcuts: "3. ショートカット",
    step_ai_privacy: "4. AI & プライバシー",
    step_finish: "5. 完了",

    welcome_headline: "Corten Desktop へようこそ",
    welcome_subline: "純粋な Rust で鍛造された、現代的で堅牢な Wayland デスクトップ環境。",

    appearance_headline: "外観と雰囲気",
    appearance_subline: "テーマ、壁紙、夜間ライトの設定を行えます。",

    shortcuts_headline: "キーボード中心の操作性",
    shortcuts_subline: "マウスに触れることなくシステム全体を操作できる主要ショートカット。",

    ai_privacy_headline: "AI & ハードウェア保護",
    ai_privacy_subline: "Spotlight 自然言語コマンドとゼロ遅延 ALSA ハードウェア監視。",

    finish_headline: "準備完了！",
    finish_subline: "設定が適用されました。Corten デスクトップを始めましょう。",

    theme_dark: "ダークモード (Dark Corten)",
    theme_dark_desc: "無煙炭の背景に温かみのある耐候性鋼の錆と銅のアクセント。",
    theme_light: "ライトモード (Brushed Steel)",
    theme_light_desc: "ヘアライン加工のステンレス鋼トーンと高コントラスト空間。",

    wallpaper_title: "壁紙の選択",
    wallpaper_patina: "Corten Dark Patina (横画面)",
    wallpaper_patina_desc: "深い錆の風合いを持つ、ミニマルな工業的抽象背景",
    wallpaper_forged: "Corten Forged Rust (縦画面 / デュアル)",
    wallpaper_forged_desc: "琥珀色の温かい輝きを放つ鍛造鋼板デザイン",
    wallpaper_keep: "現在の壁紙を維持",
    wallpaper_keep_desc: "システムで設定済みの壁紙をそのまま使用する",

    night_light_label: "🌙 夕方以降に夜間ライトを自動的に有効化する (色温度調整)",
    btn_back: "戻る",
    btn_next: "次へ →",
    btn_explore: "🚀 デスクトップを開始",
    dont_show_again: "次回起動時にこのウィザードを表示しない",
    reopen_info: "アプリメニューまたは 'corten-welcome' コマンドでいつでも再表示できます。",

    power_hub_title: "⚡ 電源とセッション管理",
    power_lock: "画面をロック",
    power_lock_desc: "セッションをロックし認証を要求します",
    power_suspend: "スリープ (サスペンド)",
    power_suspend_desc: "低消費電力の待機モードに移行します",
    power_logout: "ログアウト",
    power_logout_desc: "すべての処理を終了しログイン画面に戻ります",
    power_reboot: "再起動",
    power_reboot_desc: "システムを安全に再起動します",
    power_poweroff: "シャットダウン",
    power_poweroff_desc: "コンピューターの電源を完全に切ります",
    power_confirm_reboot: "システムを再起動しますか？",
    power_confirm_reboot_desc: "開いているすべてのウィンドウが閉じられます。",
    power_confirm_poweroff: "電源を切りますか？",
    power_confirm_poweroff_desc: "保存されていないデータは失われます。",
    btn_cancel: "キャンセル",
    btn_confirm: "確認して実行",

    weather_calendar_title: "📅 カレンダーと天気",
    weather_humidity: "湿度",
    weather_wind: "風速",
    weather_tomorrow: "明日",

    cheat_sheet_title: "⌨️ Corten キーボードショートカット一覧",
    cheat_sheet_subtitle: "マウスを使わずにすべての操作を高速に実行できます。Esc で閉じます。",
    cat_windows: "ウィンドウとワークスペース",
    cat_system: "システムと電源",
    cat_apps: "アプリケーション",
    shortcut_launcher: "Spotlight / アプリ・コマンド起動",
    shortcut_overview: "ウィンドウ一覧表示 (Exposé)",
    shortcut_terminal: "Alacritty ターミナル",
    shortcut_files: "Corten ファイルマネージャー",
    shortcut_notes: "フローティングメモ帳",
    shortcut_close: "アクティブウィンドウを閉じる",
    shortcut_fullscreen: "全画面表示切り替え",
    shortcut_power: "電源ハブを開く",
    shortcut_settings: "システム設定",

    ws_code: "💻 開発",
    ws_web: "🌐 Web",
    ws_chat: "💬 会話",
    ws_media: "🎵 メディア",

    control_center_title: "コントロールセンター",
    audio_output_device: "オーディオ出力デバイス",
    wifi_available_networks: "利用可能なWi-Fiネットワーク",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_languages_have_non_empty_strings() {
        for lang in Language::ALL {
            let t = lang.t();
            assert!(!t.brand_title.is_empty());
            assert!(!t.welcome_headline.is_empty());
            assert!(!t.power_hub_title.is_empty());
            assert!(!t.cheat_sheet_title.is_empty());
            assert!(!t.btn_explore.is_empty());
            assert!(!lang.native_name().is_empty());
            assert!(!lang.flag().is_empty());
            assert_eq!(Language::from_code(lang.code()), lang);
        }
    }

    #[test]
    fn test_language_detection() {
        assert_eq!(Language::from_code("tr_TR.UTF-8"), Language::Tr);
        assert_eq!(Language::from_code("en_US.UTF-8"), Language::En);
        assert_eq!(Language::from_code("de_DE.UTF-8"), Language::De);
        assert_eq!(Language::from_code("fr_FR.UTF-8"), Language::Fr);
        assert_eq!(Language::from_code("es_ES.UTF-8"), Language::Es);
        assert_eq!(Language::from_code("ru_RU.UTF-8"), Language::Ru);
        assert_eq!(Language::from_code("ja_JP.UTF-8"), Language::Ja);
    }
}

