mod config;
mod system;

use std::process::Command;

use config::{
    AccessibilityConfig, AiSettingsConfig, AppearanceConfig, ColorFilterChoice, CompositorSettingsConfig, DisplaySettingsConfig, InputSettingsConfig,
    LauncherSettingsConfig, NotifySettingsConfig, PanelPositionChoice, PanelSettingsConfig,
    PanelStyleChoice, PowerSettingsConfig, SoundSettingsConfig, ThemeModeChoice,
    WidgetsSettingsConfig,
};
use corten_config::ConfigManager;
use corten_theme::iced_styles::{card_style, primary_button_style, secondary_button_style};
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Transition, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES,
};
use iced::widget::{
    button, column, container, horizontal_space, row, scrollable, slider, text,
    text_input, vertical_space,
};
use iced::{
    border, event, keyboard, Alignment, Border, Color, Element, Event, Length, Shadow, Subscription,
};

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application(
        "Corten Settings",
        CortenSettingsApp::update,
        CortenSettingsApp::view,
    )
    .window(iced::window::Settings {
        size: iced::Size::new(1040.0, 720.0),
        min_size: Some(iced::Size::new(880.0, 580.0)),
        resizable: true,
        ..Default::default()
    })
    .subscription(CortenSettingsApp::subscription)
    .font(LUCIDE_FONT_BYTES)
    .run()
}

async fn reload_running_services() -> Vec<String> {
    let mut errors = Vec::new();
    let connection = match corten_ipc::get_session_connection().await {
        Ok(connection) => connection,
        Err(error) => return vec![format!("D-Bus connection: {error}")],
    };

    // corten-panel is a daemon: reload its config if running
    if let Ok(proxy) = corten_ipc::PanelControlProxy::new(&connection).await {
        if let Err(error) = proxy.reload_config().await {
            errors.push(format!("panel reload: {error}"));
        }
    }
    // corten-launcher is on-demand, but if it has an active daemon/overlay running, notify it
    if let Ok(proxy) = corten_ipc::LauncherControlProxy::new(&connection).await {
        let _ = proxy.reload_config().await;
    }
    // corten-notify is a daemon: reload its config if running
    if let Ok(proxy) = corten_ipc::NotificationControlProxy::new(&connection).await {
        if let Err(error) = proxy.reload_config().await {
            errors.push(format!("notification reload: {error}"));
        }
    }
    // corten-compositor is the core compositor daemon
    match corten_ipc::CompositorControlProxy::new(&connection).await {
        Ok(proxy) => {
            if let Err(error) = proxy.reload_config().await {
                errors.push(format!("compositor reload: {error}"));
            }
        }
        Err(error) => errors.push(format!("compositor unavailable: {error}")),
    }
    errors
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCategory {
    Appearance,
    Panel,
    Launcher,
    Widgets,
    Input,
    Displays,
    Sound,
    Bluetooth,
    Power,
    Notifications,
    Compositor,
    Shortcuts,
    Ai,
    Accessibility,
    About,
}

impl SettingsCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Görünüm ve Stil",
            Self::Panel => "Panel ve Dock",
            Self::Launcher => "Uygulama Başlatıcı",
            Self::Widgets => "Masaüstü Araçları",
            Self::Input => "Klavye ve Fare",
            Self::Displays => "Monitör ve Ekranlar",
            Self::Sound => "Ses ve Girişler",
            Self::Bluetooth => "Bluetooth ve Cihazlar",
            Self::Power => "Güç ve Pil",
            Self::Notifications => "Bildirimler ve Rahatsız Etme",
            Self::Compositor => "Pencere Yönetimi ve Efektler",
            Self::Shortcuts => "Klavye Kısayolları",
            Self::Ai => "Corten AI Asistanı",
            Self::Accessibility => "Erişilebilirlik ve Kapsayıcılık",
            Self::About => "Sistem Hakkında",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    AnimationTick,
    SelectCategory(SettingsCategory),
    SearchChanged(String),
    ToggleThemeMode(ThemeModeChoice),
    SetPatina(String),
    UpdateWallpaperPath(String),
    SelectPresetWallpaper(String),
    ChooseWallpaper,
    TestSoundOutput,
    WallpaperSelected(Result<Vec<String>, String>),
    SetPanelPosition(PanelPositionChoice),
    SetPanelStyle(PanelStyleChoice),
    TogglePanelLauncher(bool),
    TogglePanelAudio(bool),
    TogglePanelNetwork(bool),
    TogglePanelBattery(bool),
    TogglePanelAutohide(bool),
    ToggleLauncherClipboard(bool),
    SetClipboardLimit(usize),
    SetLauncherWidth(u32),
    SetLauncherDefaultStyle(String),
    SetNotifyTimeout(u64),
    SetCompositorGap(i32),
    SetCompositorLayoutMode(corten_config::LayoutMode),
    // Widgets messages
    ToggleWidgetMusic(bool),
    ToggleWidgetSysinfo(bool),
    ToggleWidgetScratchpad(bool),
    ToggleWidgetWeather(bool),
    // Input messages
    ToggleNaturalScrolling(bool),
    ToggleTapToClick(bool),
    ToggleCapsToCtrl(bool),
    ToggleVirtualKeyboard(bool),
    SetKeyboardLayout(String),
    // Display messages
    ToggleNightLight(bool),
    ToggleAutoNightLight(bool),
    ToggleVrr(bool),
    ToggleAllowTearing(bool),
    SetDisplayScale(u32),
    SetDisplayResolution(String),
    SetSecondaryScale(u32),
    SetDisplayArrangement(String),
    SetDisplayOffsetY(i32),
    SelectDisplayCard(usize),
    SetBrightness(u8),
    ToggleDisplayHdr(bool),
    SetDisplayColorSpace(String),
    SetDisplayMaxLuminance(u32),
    ChooseIccProfile,
    ClearIccProfile,
    // Sound messages
    SetOutputVolume(u8),
    SetInputVolume(u8),
    SetOutputDevice(String),
    ToggleMuteSound(bool),
    // Bluetooth messages
    ToggleBluetoothPower(bool),
    ToggleBluetoothDevice(String, bool),
    RefreshBluetooth,
    BluetoothRefreshed(bool, Vec<corten_services::bluetooth::BluetoothDevice>),
    // AI messages
    ToggleAiEnabled(bool),
    SetAiModelTier(String),
    ToggleAiGameGpuRelease(bool),
    // Power messages
    SetPowerProfile(String),
    SetScreenSleep(u32),
    // Accessibility messages
    ToggleHighContrast(bool),
    SelectColorFilter(ColorFilterChoice),
    ChangeFontScale(f32),
    ToggleLargeCursor(bool),
    ToggleHighVisFocusRing(bool),
    ToggleSoundFeedback(bool),
    ToggleScreenReader(bool),
    // Shortcut messages
    UpdateShortcut(String, String),
    ResetShortcutsToDefault,
    SaveSettings,
    ReloadResult(Vec<String>),
    KeyboardEvent(Event),
}

struct CortenSettingsApp {
    category: SettingsCategory,
    search_query: String,
    appearance: AppearanceConfig,
    panel: PanelSettingsConfig,
    launcher: LauncherSettingsConfig,
    widgets: WidgetsSettingsConfig,
    input: InputSettingsConfig,
    displays: DisplaySettingsConfig,
    sound: SoundSettingsConfig,
    power: PowerSettingsConfig,
    notify: NotifySettingsConfig,
    compositor: CompositorSettingsConfig,
    ai: AiSettingsConfig,
    accessibility: AccessibilityConfig,
    status_message: Option<String>,
    category_transition: Transition,
    selected_display: usize,
    bluetooth_devices: Vec<corten_services::bluetooth::BluetoothDevice>,
    bluetooth_powered: bool,
}

impl Default for CortenSettingsApp {
    fn default() -> Self {
        let appearance = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let panel = ConfigManager::<PanelSettingsConfig>::load_or_default("panel")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let launcher = ConfigManager::<LauncherSettingsConfig>::load_or_default("launcher")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let widgets = ConfigManager::<WidgetsSettingsConfig>::load_or_default("widgets")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let input = ConfigManager::<InputSettingsConfig>::load_or_default("input")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let displays = ConfigManager::<DisplaySettingsConfig>::load_or_default("displays")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let sound = ConfigManager::<SoundSettingsConfig>::load_or_default("sound")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let power = ConfigManager::<PowerSettingsConfig>::load_or_default("power")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let notify = ConfigManager::<NotifySettingsConfig>::load_or_default("notify")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let compositor = ConfigManager::<CompositorSettingsConfig>::load_or_default("compositor")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let ai = ConfigManager::<AiSettingsConfig>::load_or_default("ai")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let accessibility = ConfigManager::<AccessibilityConfig>::load_or_default("accessibility")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        Self {
            category: SettingsCategory::Appearance,
            search_query: String::new(),
            appearance,
            panel,
            launcher,
            widgets,
            input,
            displays,
            sound,
            power,
            notify,
            compositor,
            ai,
            accessibility,
            status_message: None,
            category_transition: Transition::new(120),
            selected_display: 0,
            bluetooth_devices: Vec::new(),
            bluetooth_powered: false,
        }
    }
}

impl CortenSettingsApp {
    fn subscription(&self) -> Subscription<Message> {
        let key_sub = event::listen().map(Message::KeyboardEvent);
        if !self.category_transition.is_complete() {
            Subscription::batch(vec![
                key_sub,
                iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::AnimationTick),
            ])
        } else {
            key_sub
        }
    }

    fn categories() -> [SettingsCategory; 15] {
        [
            SettingsCategory::Appearance,
            SettingsCategory::Panel,
            SettingsCategory::Launcher,
            SettingsCategory::Widgets,
            SettingsCategory::Input,
            SettingsCategory::Displays,
            SettingsCategory::Sound,
            SettingsCategory::Bluetooth,
            SettingsCategory::Power,
            SettingsCategory::Notifications,
            SettingsCategory::Compositor,
            SettingsCategory::Shortcuts,
            SettingsCategory::Ai,
            SettingsCategory::Accessibility,
            SettingsCategory::About,
        ]
    }

    fn current_mode(&self) -> ThemeMode {
        match self.appearance.theme_mode {
            ThemeModeChoice::Dark => ThemeMode::Dark,
            ThemeModeChoice::Light => ThemeMode::Light,
        }
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::SelectCategory(cat) => {
                self.category_transition.restart();
                self.category = cat;
                self.status_message = None;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(cat.label());
                }
                if cat == SettingsCategory::Bluetooth {
                    return iced::Task::perform(
                        async move {
                            let (powered, _) = corten_services::bluetooth::status();
                            let devices = corten_services::bluetooth::list_devices();
                            (powered, devices)
                        },
                        |(powered, devices)| Message::BluetoothRefreshed(powered, devices),
                    );
                }
            }
            Message::AnimationTick => {
                self.category_transition.tick(16);
            }
            Message::SearchChanged(q) => {
                self.search_query = q;
            }
            Message::ToggleThemeMode(mode) => {
                self.appearance.theme_mode = mode;
                self.status_message =
                    Some("Tema güncellendi. Değişiklikleri kaydetmeyi unutmayın.".into());
            }
            Message::SetPatina(patina) => {
                self.appearance.patina = patina;
                self.status_message =
                    Some("Patina rengi güncellendi. Uygulanması için 'Değişiklikleri Kaydet'e basın.".into());
            }
            Message::UpdateWallpaperPath(path) => {
                self.appearance.wallpaper_path = path;
            }
            Message::SelectPresetWallpaper(path) => {
                self.appearance.wallpaper_path = path.clone();
                if let Ok(mut cm) = corten_config::ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance") {
                    let mut cfg = cm.get().clone();
                    cfg.wallpaper_path = path;
                    let _ = cm.update(cfg);
                }
                let _ = Command::new("pkill").args(["-USR1", "corten-wallpaper"]).spawn();
                self.status_message = Some("Duvar kâğıdı seçildi ve anında uygulandı.".into());
            }
            Message::TestSoundOutput => {
                let _ = Command::new("paplay")
                    .arg("/usr/share/sounds/freedesktop/stereo/complete.oga")
                    .spawn()
                    .or_else(|_| Command::new("canberra-gtk-play").args(["-i", "bell"]).spawn());
                self.status_message = Some("🔊 Ses testi çalındı.".into());
            }
            Message::ChooseWallpaper => {
                return iced::Task::perform(
                    corten_services::portal::open_file("Duvar kâğıdı seç", false),
                    Message::WallpaperSelected,
                );
            }
            Message::WallpaperSelected(result) => match result {
                Ok(uris) => {
                    if let Some(uri) = uris.first() {
                        self.appearance.wallpaper_path =
                            uri.strip_prefix("file://").unwrap_or(uri).to_string();
                        self.status_message = Some("Duvar kâğıdı seçildi.".into());
                    }
                }
                Err(error) => self.status_message = Some(format!("Dosya seçilemedi: {error}")),
            },
            Message::SetPanelPosition(pos) => {
                self.panel.position = pos;
            }
            Message::SetPanelStyle(style) => {
                self.panel.style = style;
            }
            Message::TogglePanelLauncher(v) => self.panel.show_launcher_button = v,
            Message::TogglePanelAudio(v) => self.panel.show_audio = v,
            Message::TogglePanelNetwork(v) => self.panel.show_network = v,
            Message::TogglePanelBattery(v) => self.panel.show_battery = v,
            Message::TogglePanelAutohide(v) => self.panel.autohide = v,
            Message::ToggleLauncherClipboard(v) => self.launcher.enable_clipboard_history = v,
            Message::SetClipboardLimit(limit) => self.launcher.clipboard_history_limit = limit,
            Message::SetLauncherWidth(w) => self.launcher.width = w,
            Message::SetLauncherDefaultStyle(s) => self.launcher.style = s,
            Message::SetNotifyTimeout(t) => self.notify.default_timeout_ms = t,
            Message::SetCompositorGap(g) => self.compositor.gap = g,
            Message::SetCompositorLayoutMode(m) => self.compositor.layout_mode = m,
            // Widgets handlers
            Message::ToggleWidgetMusic(v) => self.widgets.music_enabled = v,
            Message::ToggleWidgetSysinfo(v) => self.widgets.sysinfo_enabled = v,
            Message::ToggleWidgetScratchpad(v) => self.widgets.scratchpad_enabled = v,
            Message::ToggleWidgetWeather(v) => self.widgets.weather_enabled = v,
            // Input handlers
            Message::ToggleNaturalScrolling(v) => self.input.natural_scrolling = v,
            Message::ToggleTapToClick(v) => self.input.tap_to_click = v,
            Message::ToggleCapsToCtrl(v) => self.input.remap_caps_to_ctrl = v,
            Message::ToggleVirtualKeyboard(v) => self.input.virtual_keyboard_enabled = v,
            Message::SetKeyboardLayout(l) => self.input.keyboard_layout = l,
            // Display handlers
            Message::ToggleNightLight(v) => self.displays.night_light = v,
            Message::ToggleAutoNightLight(v) => self.displays.auto_night_light = v,
            Message::ToggleVrr(v) => self.displays.vrr = v,
            Message::ToggleAllowTearing(v) => self.displays.allow_tearing = v,
            Message::SetDisplayScale(s) => self.displays.scale_pct = s,
            Message::SetDisplayResolution(resolution) => self.displays.resolution = resolution,
            Message::SetSecondaryScale(s) => self.displays.secondary_scale_pct = s,
            Message::SetDisplayArrangement(a) => self.displays.arrangement = a,
            Message::SetDisplayOffsetY(y) => self.displays.offset_y = y,
            Message::SelectDisplayCard(id) => self.selected_display = id,
            Message::SetBrightness(b) => {
                self.displays.brightness_pct = b;
                // Apply to internal/eDP panels via brightnessctl and external DDC/CI monitors
                let brightnessctl_started = std::process::Command::new("brightnessctl")
                    .args(["set", &format!("{}%", b)])
                    .spawn()
                    .is_ok();
                let ddcutil_started = std::process::Command::new("ddcutil")
                    .args(["setvcp", "10", &b.to_string()])
                    .spawn()
                    .is_ok();
                if !brightnessctl_started && !ddcutil_started {
                    self.status_message = Some(
                        "Parlaklık denetimi kullanılamıyor: brightnessctl veya ddcutil kurun."
                            .into(),
                    );
                } else {
                    self.status_message = Some("Parlaklık komutu gönderildi.".into());
                }
            }
            Message::ToggleDisplayHdr(v) => {
                self.displays.hdr_enabled = v;
                if self.accessibility.screen_reader {
                    corten_services::a11y::announce_screen_reader(if v {
                        "HDR10 ve geniş dinamik aralık etkinleştirildi"
                    } else {
                        "HDR devre dışı bırakıldı, standart SDR modu"
                    });
                }
            }
            Message::SetDisplayColorSpace(cs) => {
                self.displays.color_space = cs.clone();
                if self.accessibility.screen_reader {
                    let label = corten_services::color_management::ColorSpace::parse_str(&cs).label();
                    corten_services::a11y::announce_screen_reader(&format!("Renk uzayı: {}", label));
                }
            }
            Message::SetDisplayMaxLuminance(nits) => {
                self.displays.max_luminance_nits = Some(nits);
            }
            Message::ChooseIccProfile => {
                let sample_paths = [
                    "/usr/share/color/icc/sRGB.icc",
                    "/usr/share/color/icc/colord/sRGB.icc",
                    "/usr/share/color/icc/DisplayP3.icc",
                ];
                let chosen = sample_paths.iter().find(|p| std::path::Path::new(p).exists());
                if let Some(p) = chosen {
                    self.displays.icc_profile = Some(p.to_string());
                } else {
                    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| {
                            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".config")
                        })
                        .join("corten");
                    let _ = std::fs::create_dir_all(&config_dir);
                    let profile_path = config_dir.join("corten_display.icc");
                    let bytes = corten_services::color_management::generate_synthetic_icc_profile("Corten Kalibre Ekran Profili");
                    if std::fs::write(&profile_path, bytes).is_ok() {
                        self.displays.icc_profile = Some(profile_path.to_string_lossy().into());
                    }
                }
                if self.accessibility.screen_reader {
                    corten_services::a11y::announce_screen_reader("Monitör ICC renk profili yüklendi");
                }
            }
            Message::ClearIccProfile => {
                self.displays.icc_profile = None;
                if self.accessibility.screen_reader {
                    corten_services::a11y::announce_screen_reader("ICC profili kaldırıldı");
                }
            }
            // Sound handlers
            Message::SetOutputVolume(v) => self.sound.output_volume = v,
            Message::SetInputVolume(v) => self.sound.input_volume = v,
            Message::SetOutputDevice(device) => self.sound.output_device = device,
            Message::ToggleMuteSound(v) => self.sound.mute_output = v,
            // Bluetooth handlers
            Message::ToggleBluetoothPower(p) => {
                corten_services::bluetooth::set_powered(p);
                self.bluetooth_powered = p;
                return iced::Task::perform(
                    async move {
                        let (powered, _) = corten_services::bluetooth::status();
                        let devices = corten_services::bluetooth::list_devices();
                        (powered, devices)
                    },
                    |(powered, devices)| Message::BluetoothRefreshed(powered, devices),
                );
            }
            Message::ToggleBluetoothDevice(path, currently_connected) => {
                let _ = corten_services::bluetooth::toggle_device_connection(&path, currently_connected);
                return iced::Task::perform(
                    async move {
                        let (powered, _) = corten_services::bluetooth::status();
                        let devices = corten_services::bluetooth::list_devices();
                        (powered, devices)
                    },
                    |(powered, devices)| Message::BluetoothRefreshed(powered, devices),
                );
            }
            Message::RefreshBluetooth => {
                return iced::Task::perform(
                    async move {
                        let (powered, _) = corten_services::bluetooth::status();
                        let devices = corten_services::bluetooth::list_devices();
                        (powered, devices)
                    },
                    |(powered, devices)| Message::BluetoothRefreshed(powered, devices),
                );
            }
            Message::BluetoothRefreshed(powered, devices) => {
                self.bluetooth_powered = powered;
                self.bluetooth_devices = devices;
            }
            // AI handlers
            Message::ToggleAiEnabled(v) => self.ai.enabled = v,
            Message::SetAiModelTier(t) => self.ai.model_tier = t,
            Message::ToggleAiGameGpuRelease(v) => self.ai.auto_gpu_release_on_game = v,
            // Power handlers
            Message::SetPowerProfile(p) => self.power.power_profile = p,
            Message::SetScreenSleep(m) => self.power.screen_sleep_mins = m,
            // Accessibility handlers
            Message::ToggleHighContrast(v) => {
                self.accessibility.high_contrast = v;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(if v { "Yüksek Karşıtlık Açık" } else { "Yüksek Karşıtlık Kapalı" });
                }
            }
            Message::SelectColorFilter(f) => {
                self.accessibility.color_filter = f;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(&format!("Renk Filtresi: {:?}", f));
                }
            }
            Message::ChangeFontScale(s) => {
                self.accessibility.font_scale = s;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(&format!("Metin Ölçeği: {:.2}x", s));
                }
            }
            Message::ToggleLargeCursor(v) => {
                self.accessibility.large_cursor = v;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(if v { "Büyük İmleç Açık" } else { "Büyük İmleç Kapalı" });
                }
            }
            Message::ToggleHighVisFocusRing(v) => {
                self.accessibility.high_vis_focus_ring = v;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(if v { "Yüksek Görünürlüklü Odak Çerçevesi Açık" } else { "Yüksek Görünürlüklü Odak Çerçevesi Kapalı" });
                }
            }
            Message::ToggleSoundFeedback(v) => {
                self.accessibility.sound_feedback = v;
                if self.accessibility.screen_reader || corten_services::a11y::is_screen_reader_enabled() {
                    corten_services::a11y::announce_screen_reader(if v { "Sesli Geri Bildirim Açık" } else { "Sesli Geri Bildirim Kapalı" });
                }
            }
            Message::ToggleScreenReader(v) => {
                self.accessibility.screen_reader = v;
                corten_services::a11y::announce_screen_reader(if v { "Ekran Okuyucu Aktifleştirildi" } else { "Ekran Okuyucu Devre Dışı" });
            }
            Message::UpdateShortcut(action, chord) => {
                self.compositor.keybindings.set_binding(&action, chord);
            }
            Message::ResetShortcutsToDefault => {
                self.compositor.keybindings = corten_config::KeybindingsConfig::default();
                self.status_message = Some(
                    "Kısayollar varsayılanlara sıfırlandı. Uygulanması için 'Değişiklikleri Kaydet'e basın.".into(),
                );
            }
            Message::SaveSettings => {
                let mut save_errors = Vec::new();

                if let Err(error) = system::validate_input(&self.input) {
                    save_errors.push(format!("input: {error}"));
                }
                if let Err(error) = system::apply_sound(&self.sound) {
                    save_errors.push(format!("sound: {error}"));
                }
                if let Err(error) = system::apply_power(&self.power) {
                    save_errors.push(format!("power: {error}"));
                }
                if let Err(error) = system::apply_display(&self.displays) {
                    save_errors.push(format!("display: {error}"));
                }

                macro_rules! save_config {
                    ($config_type:ty, $name:literal, $value:expr) => {
                        if let Err(error) = ConfigManager::<$config_type>::load_or_default($name)
                            .and_then(|mut manager| manager.update($value))
                        {
                            save_errors.push(format!("{}: {}", $name, error));
                        }
                    };
                }

                save_config!(AppearanceConfig, "appearance", self.appearance.clone());
                save_config!(PanelSettingsConfig, "panel", self.panel.clone());
                save_config!(LauncherSettingsConfig, "launcher", self.launcher.clone());
                save_config!(WidgetsSettingsConfig, "widgets", self.widgets.clone());
                save_config!(InputSettingsConfig, "input", self.input.clone());
                save_config!(DisplaySettingsConfig, "displays", self.displays.clone());
                save_config!(SoundSettingsConfig, "sound", self.sound.clone());
                save_config!(PowerSettingsConfig, "power", self.power.clone());
                save_config!(NotifySettingsConfig, "notify", self.notify.clone());
                save_config!(
                    CompositorSettingsConfig,
                    "compositor",
                    self.compositor.clone()
                );
                save_config!(
                    AccessibilityConfig,
                    "accessibility",
                    self.accessibility.clone()
                );

                if save_errors.is_empty() {
                    self.status_message =
                        Some("Ayarlar kaydedildi; çalışan servisler yeniden yükleniyor...".into());
                    return iced::Task::perform(reload_running_services(), Message::ReloadResult);
                } else {
                    self.status_message =
                        Some(format!("Kaydetme başarısız: {}", save_errors.join("; ")));
                }
            }
            Message::ReloadResult(errors) => {
                self.status_message = if errors.is_empty() {
                    Some("Ayarlar kaydedildi ve çalışan servisler yeniden yüklendi.".into())
                } else {
                    Some(format!(
                        "Ayarlar kaydedildi ancak bazı servisler yeniden yüklenemedi: {}",
                        errors.join("; ")
                    ))
                };
            }
            Message::KeyboardEvent(Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(named),
                modifiers,
                ..
            })) if modifiers.alt() => {
                let categories = Self::categories();
                let current = categories
                    .iter()
                    .position(|category| *category == self.category)
                    .unwrap_or(0);
                let next = match named {
                    keyboard::key::Named::ArrowDown => {
                        next_category_index(current, 1, categories.len())
                    }
                    keyboard::key::Named::ArrowUp => {
                        next_category_index(current, -1, categories.len())
                    }
                    _ => current,
                };
                self.category = categories[next];
                self.status_message = None;
            }
            Message::KeyboardEvent(_) => {}
        }
        iced::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let patina = PatinaVariant::from_code(&self.appearance.patina);
        let palette = Palette::for_mode(self.current_mode()).with_patina(patina);
        let content_offset = (1.0 - self.category_transition.eased()) * 8.0;

        let sidebar = self.view_sidebar(palette);

        let content_view = match self.category {
            SettingsCategory::Appearance => self.view_appearance(palette),
            SettingsCategory::Panel => self.view_panel(palette),
            SettingsCategory::Launcher => self.view_launcher(palette),
            SettingsCategory::Widgets => self.view_widgets(palette),
            SettingsCategory::Input => self.view_input(palette),
            SettingsCategory::Displays => self.view_displays(palette),
            SettingsCategory::Sound => self.view_sound(palette),
            SettingsCategory::Bluetooth => self.view_bluetooth(palette),
            SettingsCategory::Power => self.view_power(palette),
            SettingsCategory::Notifications => self.view_notifications(palette),
            SettingsCategory::Compositor => self.view_compositor(palette),
            SettingsCategory::Shortcuts => self.view_shortcuts(palette),
            SettingsCategory::Ai => self.view_ai(palette),
            SettingsCategory::Accessibility => self.view_accessibility(palette),
            SettingsCategory::About => self.view_about(palette),
        };

        let footer = self.view_footer(palette);

        // Perfectly centered card column with maximum width constraint
        let content_column = container(
            column![content_view, vertical_space().height(Spacing::LG), footer,]
                .width(Length::Fill)
                .max_width(720)
                .spacing(Spacing::MD),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([Spacing::XL + content_offset, Spacing::XL]);

        let scrollable_content = scrollable(content_column)
            .width(Length::Fill)
            .height(Length::Fill);

        container(
            row![sidebar, scrollable_content]
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.background.to_iced().into()),
            text_color: Some(palette.text_primary.to_iced()),
            ..Default::default()
        })
        .into()
    }

    fn view_sidebar(&self, palette: Palette) -> Element<'_, Message> {
        let brand_badge = container(
            row![
                text(Icon::APP_WINDOW)
                    .font(LUCIDE_FONT)
                    .size(22)
                    .color(palette.primary.to_iced()),
                column![
                    text("CORTEN")
                        .size(Typography::TITLE_SM)
                        .color(palette.text_primary.to_iced()),
                    text("Sistem Ayarları")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(2),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding([Spacing::MD, Spacing::MD]);

        let nav_item = |cat: SettingsCategory, icon: char, label: &'static str| {
            let is_selected = self.category == cat;

            button(
                row![
                    text(icon).font(LUCIDE_FONT).size(17).color(if is_selected {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                    text(label).size(Typography::BODY).color(if is_selected {
                        palette.text_primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                    horizontal_space(),
                    if is_selected {
                        text("•").size(16).color(palette.primary.to_iced())
                    } else {
                        text("").size(16)
                    },
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding([Spacing::SM + 2.0, Spacing::MD])
            .on_press(Message::SelectCategory(cat))
            .style(move |_, status| {
                let (bg, border_color) = if is_selected {
                    (palette.surface_hover.to_iced(), palette.primary.to_iced())
                } else {
                    match status {
                        button::Status::Hovered | button::Status::Pressed => {
                            (palette.surface.to_iced(), palette.border.to_iced())
                        }
                        _ => (Color::TRANSPARENT, Color::TRANSPARENT),
                    }
                };

                button::Style {
                    background: Some(bg.into()),
                    text_color: palette.text_primary.to_iced(),
                    border: Border {
                        color: border_color,
                        width: if is_selected { 1.0 } else { 0.0 },
                        radius: border::Radius::from(Radius::SM),
                    },
                    shadow: Shadow::default(),
                }
            })
        };

        let search_box = text_input("Ayarlarda ara...", &self.search_query)
            .on_input(Message::SearchChanged)
            .padding([6, 10])
            .size(Typography::BODY_SM);

        let query = self.search_query.trim().to_lowercase();

        let mut nav_items_vec = Vec::new();
        let items: &[(SettingsCategory, char, &'static str)] = &[
            (SettingsCategory::Appearance, Icon::SUN, "Görünüm ve Stil"),
            (SettingsCategory::Panel, Icon::APP_WINDOW, "Panel ve Dock"),
            (
                SettingsCategory::Launcher,
                Icon::SEARCH,
                "Uygulama Başlatıcı",
            ),
            (
                SettingsCategory::Widgets,
                Icon::MUSIC,
                "Masaüstü Bileşenleri",
            ),
            (SettingsCategory::Input, Icon::KEYBOARD, "Klavye ve Fare"),
            (SettingsCategory::Displays, Icon::MONITOR, "Ekranlar"),
            (SettingsCategory::Sound, Icon::VOLUME, "Ses"),
            (SettingsCategory::Bluetooth, Icon::WIFI, "Bluetooth"),
            (SettingsCategory::Power, Icon::BATTERY, "Güç ve Pil"),
            (SettingsCategory::Notifications, Icon::BELL, "Bildirimler"),
            (
                SettingsCategory::Compositor,
                Icon::TERMINAL,
                "Pencere Yöneticisi",
            ),
            (
                SettingsCategory::Shortcuts,
                Icon::CODE,
                "Klavye Kısayolları",
            ),
            (
                SettingsCategory::Ai,
                Icon::TERMINAL,
                "Yapay Zeka (Corten AI)",
            ),
            (
                SettingsCategory::Accessibility,
                Icon::EYE,
                "Erişilebilirlik",
            ),
            (SettingsCategory::About, Icon::SHIELD, "Corten Hakkında"),
        ];

        for (cat, icon, label) in items {
            if query.is_empty() || label.to_lowercase().contains(&query) {
                nav_items_vec.push(nav_item(*cat, *icon, label).into());
            }
        }

        let nav_scrollable = scrollable(column(nav_items_vec).spacing(4))
            .width(Length::Fill)
            .height(Length::Fill);

        container(
            column![
                brand_badge,
                container(search_box).padding([0.0, Spacing::XS]),
                vertical_space().height(Spacing::XS),
                nav_scrollable,
            ]
            .width(260)
            .spacing(Spacing::SM)
            .padding([Spacing::LG, Spacing::SM]),
        )
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.surface.to_iced().into()),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::default(),
            },
            ..Default::default()
        })
        .into()
    }

    /// Responsive setting item inside a settings group card
    fn setting_item<'a>(
        title: &'static str,
        subtitle: &'static str,
        control: impl Into<Element<'a, Message>>,
        palette: Palette,
    ) -> Element<'a, Message> {
        let info = column![
            text(title)
                .size(Typography::BODY)
                .color(palette.text_primary.to_iced()),
            text(subtitle)
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(3)
        .width(Length::Fill);

        let ctrl = container(control.into())
            .width(Length::Shrink)
            .align_x(Alignment::End)
            .align_y(Alignment::Center);

        container(
            row![info, horizontal_space(), ctrl]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .padding([Spacing::MD, Spacing::LG])
        .width(Length::Fill)
        .into()
    }

    /// Modern grouped settings card with subtle dividers
    fn settings_group<'a>(
        items: Vec<Element<'a, Message>>,
        palette: Palette,
    ) -> Element<'a, Message> {
        let mut col = column![].spacing(0);
        let len = items.len();
        for (i, item) in items.into_iter().enumerate() {
            col = col.push(item);
            if i + 1 < len {
                col = col.push(
                    container(row![])
                        .height(Length::Fixed(1.0))
                        .width(Length::Fill)
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
                            ..Default::default()
                        }),
                );
            }
        }
        container(col)
            .width(Length::Fill)
            .style(move |_| card_style(palette))
            .into()
    }

    /// Responsive setting row (backwards-compatible single-item group)
    fn setting_row<'a>(
        title: &'static str,
        subtitle: &'static str,
        control: impl Into<Element<'a, Message>>,
        palette: Palette,
    ) -> Element<'a, Message> {
        Self::settings_group(vec![Self::setting_item(title, subtitle, control, palette)], palette)
    }

    /// Modern iOS / macOS / Fluent style interactive pill switch
    fn pill_switch<'a>(
        is_active: bool,
        on_toggle: Message,
        palette: Palette,
    ) -> Element<'a, Message> {
        let (track_color, border_color) = if is_active {
            (palette.primary.to_iced(), palette.primary.to_iced())
        } else {
            (
                Color::from_rgba(0.16, 0.18, 0.23, 0.95),
                Color::from_rgba(0.24, 0.28, 0.35, 0.8),
            )
        };

        let thumb = container(row![])
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(if is_active {
                    Color::WHITE
                } else {
                    Color::from_rgb(0.72, 0.76, 0.82)
                })),
                border: Border {
                    radius: border::Radius::from(9.0),
                    ..Default::default()
                },
                shadow: Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                    offset: iced::Vector::new(0.0, 1.5),
                    blur_radius: 3.0,
                },
                ..Default::default()
            });

        let switch_content = if is_active {
            row![horizontal_space().width(Length::Fill), thumb]
        } else {
            row![thumb, horizontal_space().width(Length::Fill)]
        }
        .align_y(Alignment::Center)
        .width(Length::Fill);

        button(
            container(switch_content)
                .width(Length::Fixed(46.0))
                .height(Length::Fixed(24.0))
                .padding(2.0)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(track_color)),
                    border: Border {
                        color: border_color,
                        width: 1.0,
                        radius: border::Radius::from(12.0),
                    },
                    ..Default::default()
                }),
        )
        .padding(0)
        .on_press(on_toggle)
        .style(|_, _| button::Style {
            background: None,
            ..Default::default()
        })
        .into()
    }

    fn segment_button<'a>(
        label: &'static str,
        selected: bool,
        msg: Message,
        palette: Palette,
    ) -> Element<'a, Message> {
        button(text(label).size(Typography::BODY_SM).color(if selected {
            Color::WHITE
        } else {
            palette.text_muted.to_iced()
        }))
        .padding([Spacing::XS + 2.0, Spacing::MD])
        .on_press(msg)
        .style(move |_, status| {
            let bg = if selected {
                palette.primary.to_iced()
            } else {
                match status {
                    button::Status::Hovered => palette.surface.to_iced(),
                    _ => palette.background.to_iced(),
                }
            };
            button::Style {
                background: Some(bg.into()),
                text_color: if selected {
                    Color::WHITE
                } else {
                    palette.text_muted.to_iced()
                },
                border: Border {
                    color: if selected {
                        palette.primary.to_iced()
                    } else {
                        palette.border.to_iced()
                    },
                    width: 1.0,
                    radius: border::Radius::from(Radius::SM),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
    }

    fn view_appearance(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Görünüm ve Tema")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Masaüstü stilini, açık/koyu temayı ve duvar kâğıdını özelleştirin")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let dark_selected = self.appearance.theme_mode == ThemeModeChoice::Dark;

        let theme_toggle = row![
            Self::segment_button(
                "Dark Anthracite",
                dark_selected,
                Message::ToggleThemeMode(ThemeModeChoice::Dark),
                palette,
            ),
            Self::segment_button(
                "Warm Light",
                !dark_selected,
                Message::ToggleThemeMode(ThemeModeChoice::Light),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let current_patina = PatinaVariant::from_code(&self.appearance.patina);
        let patina_toggle = row![
            Self::segment_button(
                "Rust (Pas)",
                current_patina == PatinaVariant::Rust,
                Message::SetPatina("rust".into()),
                palette,
            ),
            Self::segment_button(
                "Verdigris (Zümrüt)",
                current_patina == PatinaVariant::Verdigris,
                Message::SetPatina("verdigris".into()),
                palette,
            ),
            Self::segment_button(
                "Titanium (Çelik)",
                current_patina == PatinaVariant::Titanium,
                Message::SetPatina("titanium".into()),
                palette,
            ),
            Self::segment_button(
                "Molten (Altın)",
                current_patina == PatinaVariant::Molten,
                Message::SetPatina("molten".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let wp_input = text_input(
            "e.g. /home/user/Pictures/wallpaper.jpg",
            &self.appearance.wallpaper_path,
        )
        .on_input(Message::UpdateWallpaperPath)
        .padding([Spacing::XS + 2.0, Spacing::SM])
        .width(Length::Fill);
        let wallpaper_control = row![
            wp_input,
            button(text("Dosya seç")).on_press(Message::ChooseWallpaper),
        ]
        .spacing(Spacing::SM)
        .width(Length::Fill);

        let current_wp = &self.appearance.wallpaper_path;
        let p1_path = if std::path::Path::new("/usr/share/backgrounds/corten/corten_dark_landscape.jpg").exists() {
            "/usr/share/backgrounds/corten/corten_dark_landscape.jpg"
        } else {
            "assets/wallpapers/corten_dark_landscape.jpg"
        };
        let p2_path = if std::path::Path::new("/usr/share/backgrounds/corten/corten_dark_portrait.jpg").exists() {
            "/usr/share/backgrounds/corten/corten_dark_portrait.jpg"
        } else {
            "assets/wallpapers/corten_dark_portrait.jpg"
        };

        let preset_btn = |name: &'static str, path: &'static str, is_active: bool| {
            button(
                row![
                    text(if is_active { "●" } else { "○" })
                        .size(11.0)
                        .color(if is_active { palette.primary.to_iced() } else { palette.text_muted.to_iced() }),
                    text(name)
                        .size(Typography::BODY_SM)
                        .color(if is_active { palette.primary.to_iced() } else { palette.text_primary.to_iced() }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .style(move |_, status| {
                let bg = if is_active {
                    iced::Color::from_rgba(0.85, 0.42, 0.26, 0.18)
                } else if matches!(status, button::Status::Hovered) {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04)
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: if is_active { palette.primary.to_iced() } else { palette.border.to_iced() },
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::SelectPresetWallpaper(path.to_string()))
        };

        let wallpaper_presets = row![
            preset_btn("Corten Dark Patina (Yatay)", p1_path, current_wp.contains("landscape")),
            preset_btn("Corten Forged Rust (Dikey)", p2_path, current_wp.contains("portrait")),
        ]
        .spacing(Spacing::SM);

        // Interactive Live Miniature Desktop Canvas Preview
        let preview_dock = container(
            row![
                text(Icon::APP_WINDOW)
                    .font(LUCIDE_FONT)
                    .size(10.0)
                    .color(palette.primary.to_iced()),
                text(Icon::TERMINAL)
                    .font(LUCIDE_FONT)
                    .size(10.0)
                    .color(palette.text_primary.to_iced()),
                text(Icon::CODE)
                    .font(LUCIDE_FONT)
                    .size(10.0)
                    .color(palette.text_primary.to_iced()),
                text(Icon::GLOBE)
                    .font(LUCIDE_FONT)
                    .size(10.0)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
        )
        .padding([3, 10])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(palette.surface_hover.to_iced())),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let preview_window = container(
            column![
                row![
                    text("terminal ~ corten")
                        .size(9.0)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    row![
                        // Minimize (-)
                        container(text("-").size(7.0).color(palette.primary.to_iced()))
                            .width(Length::Fixed(8.0))
                            .height(Length::Fixed(8.0))
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        // Maximize (□)
                        container(text("□").size(6.0).color(palette.primary.to_iced()))
                            .width(Length::Fixed(8.0))
                            .height(Length::Fixed(8.0))
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        // Close (✕)
                        container(text("✕").size(6.0).color(iced::Color::WHITE))
                            .width(Length::Fixed(8.0))
                            .height(Length::Fixed(8.0))
                            .center_x(Length::Fill)
                            .center_y(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(palette.primary.to_iced())),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    ]
                    .spacing(3)
                    .align_y(Alignment::Center),
                ]
                .width(Length::Fill)
                .align_y(Alignment::Center),
                text("corten-desktop ~ $ cargo run")
                    .size(Typography::CAPTION)
                    .color(palette.primary.to_iced()),
            ]
            .spacing(4),
        )
        .padding(6)
        .width(Length::Fixed(160.0))
        .height(Length::Fixed(80.0))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(palette.surface.to_iced())),
            border: Border {
                color: palette.primary.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let mini_desktop_canvas = container(
            column![
                row![
                    text("12:00 PM")
                        .size(9.0)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    row![
                        text(Icon::WIFI)
                            .font(LUCIDE_FONT)
                            .size(9.0)
                            .color(palette.primary.to_iced()),
                        text(Icon::VOLUME)
                            .font(LUCIDE_FONT)
                            .size(9.0)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(4),
                ]
                .width(Length::Fill)
                .align_y(Alignment::Center),
                preview_window,
                vertical_space().height(4),
                container(preview_dock)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            ]
            .spacing(6)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .padding(8)
        .width(Length::Fixed(240.0))
        .height(Length::Fixed(155.0))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(palette.background.to_iced())),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            shadow: Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Arayüz Teması",
                        "Koyu antrasit ve sıcak krem açık tema arasında seçim yapın",
                        theme_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Sistem Patina & Vurgu Rengi",
                        "Pencere sınırları, kart ışıltıları ve butonlar için metalik patina teması",
                        patina_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Canlı Küçük Önizleme",
                        "Etkin renk paletini ve pencere stilini anlık olarak önizleyin",
                        mini_desktop_canvas,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Masaüstü Duvar Kâğıdı",
                        "Kilit ekranı ve masaüstü arka plan görseli (önayarlı kartlara tıklayarak anında uygulayabilirsiniz)",
                        column![wallpaper_control, wallpaper_presets].spacing(Spacing::XS),
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_panel(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Panel ve Dock")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Konumu, dock görünümünü ve sistem bileşenlerini yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let is_top = self.panel.position == PanelPositionChoice::Top;
        let pos_control = row![
            Self::segment_button(
                "Üst Panel",
                is_top,
                Message::SetPanelPosition(PanelPositionChoice::Top),
                palette,
            ),
            Self::segment_button(
                "Alt Dock",
                !is_top,
                Message::SetPanelPosition(PanelPositionChoice::Bottom),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let style_control = row![
            Self::segment_button(
                "Yüzen",
                self.panel.style == PanelStyleChoice::Floating,
                Message::SetPanelStyle(PanelStyleChoice::Floating),
                palette,
            ),
            Self::segment_button(
                "Adacıklar",
                self.panel.style == PanelStyleChoice::Islands,
                Message::SetPanelStyle(PanelStyleChoice::Islands),
                palette,
            ),
            Self::segment_button(
                "Tam Genişlik",
                self.panel.style == PanelStyleChoice::Flush,
                Message::SetPanelStyle(PanelStyleChoice::Flush),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let launcher_cb = Self::pill_switch(
            self.panel.show_launcher_button,
            Message::TogglePanelLauncher(!self.panel.show_launcher_button),
            palette,
        );
        let audio_cb = Self::pill_switch(
            self.panel.show_audio,
            Message::TogglePanelAudio(!self.panel.show_audio),
            palette,
        );
        let network_cb = Self::pill_switch(
            self.panel.show_network,
            Message::TogglePanelNetwork(!self.panel.show_network),
            palette,
        );
        let battery_cb = Self::pill_switch(
            self.panel.show_battery,
            Message::TogglePanelBattery(!self.panel.show_battery),
            palette,
        );
        let autohide_cb = Self::pill_switch(
            self.panel.autohide,
            Message::TogglePanelAutohide(!self.panel.autohide),
            palette,
        );

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Panel Konumu",
                        "Panelin üst kenarda mı yoksa alt dock olarak mı duracağını seçin",
                        pos_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Görsel Geometri",
                        "Yüzen kart dock, kapsül adacıklar veya tam genişlikte çubuk",
                        style_control,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Corten Başlatıcı Düğmesi",
                        "Dock'un solunda Corten orb amblem düğmesini göster",
                        launcher_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Ses ve Medya Bileşeni",
                        "Gerçek zamanlı PipeWire ses düzeyini ve VU metre göstergesini göster",
                        audio_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Ağ ve Bağlantı",
                        "NetworkManager durum simgesini göster (Wi-Fi / Ethernet)",
                        network_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Pil ve Güç",
                        "Taşınabilir sistemlerde pil seviyesini ve güç durumunu göster",
                        battery_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Akıllı Otomatik Gizleme (Autohide)",
                        "Ekran alanını artırmak için dock'u otomatik gizle",
                        autohide_cb,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_launcher(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Uygulama Başlatıcı ve Spotlight")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Spotlight boyutlarını, arama tercihlerini ve pano yöneticisini yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let clip_cb = Self::pill_switch(
            self.launcher.enable_clipboard_history,
            Message::ToggleLauncherClipboard(!self.launcher.enable_clipboard_history),
            palette,
        );

        let width_control = row![
            Self::segment_button(
                "600px (Varsayılan)",
                self.launcher.width == 600,
                Message::SetLauncherWidth(600),
                palette,
            ),
            Self::segment_button(
                "750px (Geniş)",
                self.launcher.width == 750,
                Message::SetLauncherWidth(750),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let style_control = row![
            Self::segment_button(
                "Başlat Menüsü (Start)",
                self.launcher.style == "startmenu",
                Message::SetLauncherDefaultStyle("startmenu".into()),
                palette,
            ),
            Self::segment_button(
                "Spotlight (Kompakt)",
                self.launcher.style == "spotlight",
                Message::SetLauncherDefaultStyle("spotlight".into()),
                palette,
            ),
            Self::segment_button(
                "Uygulama Panosu (Izgara)",
                self.launcher.style == "dashboard",
                Message::SetLauncherDefaultStyle("dashboard".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let clip_limit_control = row![
            Self::segment_button(
                "25 Öğe",
                self.launcher.clipboard_history_limit == 25,
                Message::SetClipboardLimit(25),
                palette,
            ),
            Self::segment_button(
                "50 Öğe",
                self.launcher.clipboard_history_limit == 50,
                Message::SetClipboardLimit(50),
                palette,
            ),
            Self::segment_button(
                "100 Öğe",
                self.launcher.clipboard_history_limit == 100,
                Message::SetClipboardLimit(100),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Varsayılan Başlatıcı Stili",
                        "Kompakt Başlat Menüsü, Spotlight veya geniş Uygulama Panosu arasında seçim yapın",
                        style_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Spotlight Genişliği",
                        "Ortalanmış Spotlight arama penceresinin piksel genişliği",
                        width_control,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Pano Yöneticisi Entegrasyonu",
                        "Kopyalanan metin ve görsellerin pano geçmişine kaydedilmesini sağlar",
                        clip_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Pano Geçmişi Derinliği",
                        "Panoda saklanacak maksimum metin ve görsel kopyalama sayısı",
                        clip_limit_control,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_widgets(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Masaüstü Bileşenleri")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Masaüstü araçlarını, ses görselleştiricisini ve not defterini yönetin")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let music_cb = Self::pill_switch(
            self.widgets.music_enabled,
            Message::ToggleWidgetMusic(!self.widgets.music_enabled),
            palette,
        );
        let sysinfo_cb = Self::pill_switch(
            self.widgets.sysinfo_enabled,
            Message::ToggleWidgetSysinfo(!self.widgets.sysinfo_enabled),
            palette,
        );
        let weather_cb = Self::pill_switch(
            self.widgets.weather_enabled,
            Message::ToggleWidgetWeather(!self.widgets.weather_enabled),
            palette,
        );
        let scratchpad_cb = Self::pill_switch(
            self.widgets.scratchpad_enabled,
            Message::ToggleWidgetScratchpad(!self.widgets.scratchpad_enabled),
            palette,
        );

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Müzik & Ses Dalga Görselleştiricisi",
                        "MPRIS medya oynatıcılarına duyarlı canlı ekolayzer widget'ı",
                        music_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Donanım & Sistem İzleyici",
                        "Masaüstünde anlık CPU ve RAM kullanım göstergesi",
                        sysinfo_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Masaüstü Hava Durumu",
                        "Gerçek zamanlı hava durumu ve sıcaklık bilgisi",
                        weather_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Masaüstü Not Defteri",
                        "Masaüstünde kalıcı hızlı notlar ve yapılacaklar listesi",
                        scratchpad_cb,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_input(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Klavye ve Fare")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Klavye düzenini, touchpad hareketlerini ve tuş dönüşümlerini yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let is_tr = self.input.keyboard_layout == "tr";
        let layout_control = row![
            Self::segment_button(
                "Turkish (TR-Q)",
                is_tr,
                Message::SetKeyboardLayout("tr".into()),
                palette,
            ),
            Self::segment_button(
                "English (US)",
                !is_tr,
                Message::SetKeyboardLayout("us".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let natural_cb = Self::pill_switch(
            self.input.natural_scrolling,
            Message::ToggleNaturalScrolling(!self.input.natural_scrolling),
            palette,
        );
        let tap_cb = Self::pill_switch(
            self.input.tap_to_click,
            Message::ToggleTapToClick(!self.input.tap_to_click),
            palette,
        );
        let caps_cb = Self::pill_switch(
            self.input.remap_caps_to_ctrl,
            Message::ToggleCapsToCtrl(!self.input.remap_caps_to_ctrl),
            palette,
        );
        let vkey_cb = Self::pill_switch(
            self.input.virtual_keyboard_enabled,
            Message::ToggleVirtualKeyboard(!self.input.virtual_keyboard_enabled),
            palette,
        );

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Klavye Düzeni",
                        "Terminal ve uygulamalarda yazım için geçerli XKB düzeni",
                        layout_control,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Doğal Kaydırma (Natural Scrolling)",
                        "Touchpad ve fare tekerinde parmak hareketini takip et",
                        natural_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Dokunarak Tıklama (Tap to Click)",
                        "Touchpad yüzeyine hafifçe dokunarak birincil tıklama yap",
                        tap_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "CapsLock tuşunu Control / Esc yap",
                        "Hızlı terminal ve Vim iş akışları için ergonomik tuş eşlemesi",
                        caps_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Sanal Ekran Klavyesi",
                        "Kilit ekranında ve dokunmatik ekranlarda sanal klavye",
                        vkey_cb,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_displays(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Ekranlar")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Ekran ölçeğini, yenileme hızını ve görsel yerleşim düzenini yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        // Visual Interactive Multi-Monitor Arrangement Canvas
        let detected = system::detect_drm_outputs();
        let disp1_name = detected.first().map(|d| d.name.clone()).unwrap_or_else(|| "DP-1".into());
        let disp2_name = detected.get(1).map(|d| d.name.clone()).unwrap_or_else(|| "HDMI-1".into());

        let is_disp1_selected = self.selected_display == 0;
        let is_disp2_selected = self.selected_display == 1;

        let disp1_label = format!("{} (Aktif)", disp1_name);
        let disp2_label = if detected.get(1).map(|d| d.is_connected).unwrap_or(false) {
            format!("{} (Bağlı)", disp2_name)
        } else {
            format!("{} (Bağlı Değil)", disp2_name)
        };

        let disp1_card = button(
            container(
                column![
                    row![
                        container(
                            text("1")
                                .size(11.0)
                                .color(Color::WHITE),
                        )
                        .padding([2, 6])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: Border {
                                radius: border::Radius::from(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        text(disp1_label)
                            .size(11.0)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center),
                    vertical_space().height(4),
                    text(&self.displays.resolution)
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                    text(format!("Ölçek: %{}", self.displays.scale_pct))
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(2)
                .align_x(Alignment::Center),
            )
            .width(Length::Fixed(150.0))
            .height(Length::Fixed(92.0))
            .padding(Spacing::SM)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        )
        .style(move |_, status| {
            let border_color = if is_disp1_selected {
                palette.primary.to_iced()
            } else if status == button::Status::Hovered {
                palette.text_muted.to_iced()
            } else {
                palette.border.to_iced()
            };
            let bg = if is_disp1_selected {
                palette.surface_hover.to_iced()
            } else {
                palette.surface.to_iced()
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: Border {
                    color: border_color,
                    width: if is_disp1_selected { 2.0 } else { 1.0 },
                    radius: border::Radius::from(Radius::SM),
                },
                shadow: Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.2),
                    offset: iced::Vector::new(0.0, 2.0),
                    blur_radius: 8.0,
                },
                ..Default::default()
            }
        })
        .on_press(Message::SelectDisplayCard(0));

        let disp2_card = button(
            container(
                column![
                    row![
                        container(
                            text("2")
                                .size(11.0)
                                .color(Color::WHITE),
                        )
                        .padding([2, 6])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: Border {
                                radius: border::Radius::from(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        text(disp2_label)
                            .size(11.0)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center),
                    vertical_space().height(4),
                    text("Harici Ekran")
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                    text(format!("Ölçek: %{}", self.displays.secondary_scale_pct))
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(2)
                .align_x(Alignment::Center),
            )
            .width(Length::Fixed(150.0))
            .height(Length::Fixed(92.0))
            .padding(Spacing::SM)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        )
        .style(move |_, status| {
            let border_color = if is_disp2_selected {
                palette.primary.to_iced()
            } else if status == button::Status::Hovered {
                palette.text_muted.to_iced()
            } else {
                palette.border.to_iced()
            };
            let bg = if is_disp2_selected {
                palette.surface_hover.to_iced()
            } else {
                palette.surface.to_iced()
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: Border {
                    color: border_color,
                    width: if is_disp2_selected { 2.0 } else { 1.0 },
                    radius: border::Radius::from(Radius::SM),
                },
                shadow: Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.2),
                    offset: iced::Vector::new(0.0, 2.0),
                    blur_radius: 8.0,
                },
                ..Default::default()
            }
        })
        .on_press(Message::SelectDisplayCard(1));

        let visual_offset = (self.displays.offset_y as f32 / 25.0).clamp(-35.0, 35.0);
        let disp2_canvas_card = container(disp2_card).padding(iced::Padding {
            top: if visual_offset > 0.0 { visual_offset } else { 0.0 },
            right: 0.0,
            bottom: if visual_offset < 0.0 { -visual_offset } else { 0.0 },
            left: 0.0,
        });

        let visual_stage: Element<'_, Message> = match self.displays.arrangement.as_str() {
            "extend-left" => row![disp2_canvas_card, disp1_card].spacing(16).align_y(Alignment::Center).into(),
            "extend-above" => column![disp2_canvas_card, disp1_card].spacing(12).align_x(Alignment::Center).into(),
            "extend-below" => column![disp1_card, disp2_canvas_card].spacing(12).align_x(Alignment::Center).into(),
            "mirror" => row![
                disp1_card,
                text("⇄")
                    .size(18.0)
                    .color(palette.primary.to_iced()),
                disp2_canvas_card
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
            _ => row![disp1_card, disp2_canvas_card].spacing(16).align_y(Alignment::Center).into(), // default extend-right
        };

        let arranger_container = container(
            column![
                text("Görsel Ekran Konumlandırma & KScreen Tuvali")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
                container(visual_stage)
                    .width(Length::Fill)
                    .height(Length::Fixed(150.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
                row![
                    Self::segment_button(
                        "← Sol",
                        self.displays.arrangement == "extend-left",
                        Message::SetDisplayArrangement("extend-left".into()),
                        palette,
                    ),
                    Self::segment_button(
                        "Sağ →",
                        self.displays.arrangement == "extend-right",
                        Message::SetDisplayArrangement("extend-right".into()),
                        palette,
                    ),
                    Self::segment_button(
                        "↑ Üst",
                        self.displays.arrangement == "extend-above",
                        Message::SetDisplayArrangement("extend-above".into()),
                        palette,
                    ),
                    Self::segment_button(
                        "↓ Alt",
                        self.displays.arrangement == "extend-below",
                        Message::SetDisplayArrangement("extend-below".into()),
                        palette,
                    ),
                    Self::segment_button(
                        "⇄ Ayna",
                        self.displays.arrangement == "mirror",
                        Message::SetDisplayArrangement("mirror".into()),
                        palette,
                    ),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                vertical_space().height(4),
                row![
                    text(format!("Düşey Konum Ofseti: {} px", self.displays.offset_y))
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    Self::segment_button(
                        "Üst (0 px)",
                        self.displays.offset_y == 0,
                        Message::SetDisplayOffsetY(0),
                        palette,
                    ),
                    Self::segment_button(
                        "Ortala (180 px)",
                        self.displays.offset_y == 180,
                        Message::SetDisplayOffsetY(180),
                        palette,
                    ),
                    Self::segment_button(
                        "Alt (360 px)",
                        self.displays.offset_y == 360,
                        Message::SetDisplayOffsetY(360),
                        palette,
                    ),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                slider(-1080..=1080, self.displays.offset_y, Message::SetDisplayOffsetY)
                    .width(Length::Fill)
                    .style(corten_theme::iced_styles::capsule_slider_style(palette)),
            ]
            .spacing(Spacing::SM)
            .width(Length::Fill),
        )
        .padding(Spacing::MD)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(palette.background.to_iced())),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            ..Default::default()
        });

        let scale_control = row![
            Self::segment_button(
                "100% (Standard)",
                self.displays.scale_pct == 100,
                Message::SetDisplayScale(100),
                palette,
            ),
            Self::segment_button(
                "125% (HiDPI)",
                self.displays.scale_pct == 125,
                Message::SetDisplayScale(125),
                palette,
            ),
            Self::segment_button(
                "150% (Retina)",
                self.displays.scale_pct == 150,
                Message::SetDisplayScale(150),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let resolution_input = text_input("Örn. 1920x1080@60Hz", &self.displays.resolution)
            .on_input(Message::SetDisplayResolution)
            .padding([Spacing::SM, Spacing::MD]);

        let night_light_cb = Self::pill_switch(
            self.displays.night_light,
            Message::ToggleNightLight(!self.displays.night_light),
            palette,
        );
        let auto_night_light_cb = Self::pill_switch(
            self.displays.auto_night_light,
            Message::ToggleAutoNightLight(!self.displays.auto_night_light),
            palette,
        );
        let vrr_cb = Self::pill_switch(
            self.displays.vrr,
            Message::ToggleVrr(!self.displays.vrr),
            palette,
        );
        let tearing_cb = Self::pill_switch(
            self.displays.allow_tearing,
            Message::ToggleAllowTearing(!self.displays.allow_tearing),
            palette,
        );

        let cur_cs = corten_services::color_management::ColorSpace::parse_str(&self.displays.color_space);
        let color_space_picker = row![
            Self::segment_button(
                "sRGB (Standart)",
                cur_cs == corten_services::color_management::ColorSpace::Srgb,
                Message::SetDisplayColorSpace("srgb".into()),
                palette,
            ),
            Self::segment_button(
                "DCI-P3 (Geniş Gamut)",
                cur_cs == corten_services::color_management::ColorSpace::DciP3,
                Message::SetDisplayColorSpace("dci-p3".into()),
                palette,
            ),
            Self::segment_button(
                "BT.2020 (HDR)",
                cur_cs == corten_services::color_management::ColorSpace::Bt2020,
                Message::SetDisplayColorSpace("bt2020".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let hdr_cb = Self::pill_switch(
            self.displays.hdr_enabled,
            Message::ToggleDisplayHdr(!self.displays.hdr_enabled),
            palette,
        );

        let max_nits = self.displays.max_luminance_nits.unwrap_or(400);
        let max_luminance_slider = row![
            slider(300..=2000, max_nits, Message::SetDisplayMaxLuminance)
                .width(Length::Fill)
                .style(corten_theme::iced_styles::capsule_slider_style(palette)),
            text(format!("{} nits", max_nits))
                .size(Typography::BODY_SM)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let icc_status = if let Some(ref path) = self.displays.icc_profile {
            let label = if let Ok(info) = corten_services::color_management::load_icc_profile_file(path) {
                format!("{} ({})", info.description, info.color_space)
            } else {
                path.split('/').next_back().unwrap_or("Özel Profil").to_string()
            };
            row![
                text(label)
                    .size(Typography::BODY_SM)
                    .color(palette.primary.to_iced()),
                button(text("Kaldır").size(11.0))
                    .padding([4, 8])
                    .style(move |_, status| {
                        button::Style {
                            background: Some(iced::Background::Color(if status == button::Status::Hovered {
                                palette.danger.to_iced()
                            } else {
                                palette.surface.to_iced()
                            })),
                            border: Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::ClearIccProfile),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
        } else {
            row![
                text("Varsayılan Donanım Profili (EDID)")
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
                button(text("Profil Yükle (.icc)").size(11.0))
                    .padding([4, 8])
                    .style(move |_, status| {
                        button::Style {
                            background: Some(iced::Background::Color(if status == button::Status::Hovered {
                                palette.primary.to_iced()
                            } else {
                                palette.surface.to_iced()
                            })),
                            border: Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::ChooseIccProfile),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
        };

        let brightness_slider = row![
            slider(
                10..=100,
                self.displays.brightness_pct,
                Message::SetBrightness
            )
            .width(Length::Fill)
            .style(corten_theme::iced_styles::capsule_slider_style(palette)),
            text(format!("{}%", self.displays.brightness_pct))
                .size(Typography::BODY_SM)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let secondary_scale_control = row![
            Self::segment_button(
                "100%",
                self.displays.secondary_scale_pct == 100,
                Message::SetSecondaryScale(100),
                palette,
            ),
            Self::segment_button(
                "125%",
                self.displays.secondary_scale_pct == 125,
                Message::SetSecondaryScale(125),
                palette,
            ),
            Self::segment_button(
                "150%",
                self.displays.secondary_scale_pct == 150,
                Message::SetSecondaryScale(150),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            arranger_container,
            vertical_space().height(Spacing::XS),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Birincil Ekran Ölçeği (eDP-1 / Dizüstü)",
                        "Dahili birincil ekranın kesirli arayüz ölçeğini ayarlayın",
                        scale_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Çözünürlük ve Yenileme Hızı",
                        "DRM tarafından desteklenen biçim: genişlik x yükseklik @ Hz",
                        resolution_input,
                        palette,
                    ),
                    Self::setting_item(
                        "İkincil Ekran Ölçeği (Harici)",
                        "Çift ekran kurulumunda çıkışa özel bağımsız ölçek katsayısı",
                        secondary_scale_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Ekran Parlaklığı",
                        "Donanım arka aydınlatma parlaklığı kontrolü",
                        brightness_slider,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Gece Işığı (Night Light - Mavi Işık Filtresi)",
                        "Göz yorgunluğunu azaltmak için ekran renk sıcaklığını sıcak tonlara kaydır",
                        night_light_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Otomatik Gün Batımı / Gece Işığı Zamanlayıcısı",
                        "Günün saatine göre (19:00 - 07:00) mavi ışık filtresini otomatik olarak açıp kapatır",
                        auto_night_light_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "VRR (FreeSync / Değişken Tazeleme Hızı)",
                        "Desteklenen monitörlerde ekran yenileme hızını dinamik FPS ile senkronize eder",
                        vrr_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Düşük Gecikmeli Oyun Modu (Tearing / V-Sync Bypass)",
                        "Rekabetçi oyunlarda V-Sync giriş gecikmesini (input lag) tamamen ortadan kaldırır",
                        tearing_cb,
                        palette,
                    ),
                ],
                palette,
            ),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Renk Uzayı ve Gamut (Color Space)",
                        "Monitör panelinin renk kapsamı (sRGB / DCI-P3 / BT.2020)",
                        color_space_picker,
                        palette,
                    ),
                    Self::setting_item(
                        "HDR10 / ST.2084 PQ Çıkışı",
                        "10-bit SMPTE ST 2084 dinamik aralık ve geniş renk gamutu DRM metadata sinyali",
                        hdr_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "HDR Tepe Parlaklığı (Peak Luminance)",
                        "Monitör donanımının desteklediği maksimum tepe parlaklık düzeyi (cd/m²)",
                        max_luminance_slider,
                        palette,
                    ),
                    Self::setting_item(
                        "Monitör ICC Kalibrasyon Profili",
                        "Donanımsal renk doğruluğu ve ICC renk yönetim profili",
                        icc_status,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_sound(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Ses")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Ana ses düzeyini, PipeWire yönlendirmesini ve mikrofon seviyesini yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let out_vol_slider = row![
            slider(0..=100, self.sound.output_volume, Message::SetOutputVolume)
                .width(Length::Fixed(150.0))
                .style(corten_theme::iced_styles::capsule_slider_style(palette)),
            text(format!("{}%", self.sound.output_volume))
                .size(Typography::BODY_SM)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let test_btn = button(
            row![
                text(Icon::VOLUME).font(LUCIDE_FONT).size(12.0),
                text("Sesi Test Et").size(Typography::CAPTION),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(palette.surface_hover.to_iced())),
            text_color: palette.primary.to_iced(),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::SM),
            },
            ..Default::default()
        })
        .on_press(Message::TestSoundOutput);

        let out_vol_row = row![
            out_vol_slider,
            test_btn,
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let in_vol_slider = row![
            slider(0..=100, self.sound.input_volume, Message::SetInputVolume)
                .width(Length::Fixed(180.0))
                .style(corten_theme::iced_styles::capsule_slider_style(palette)),
            text(format!("{}%", self.sound.input_volume))
                .size(Typography::BODY_SM)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let mute_cb = Self::pill_switch(
            self.sound.mute_output,
            Message::ToggleMuteSound(!self.sound.mute_output),
            palette,
        );
        let output_device = text_input(
            "Varsayılan aygıt veya PipeWire node adı",
            &self.sound.output_device,
        )
        .on_input(Message::SetOutputDevice)
        .width(Length::Fixed(240.0))
        .padding([Spacing::SM, Spacing::MD]);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Çıkış Ses Düzeyi",
                        "Varsayılan ses çıkışındaki ana sistem oynatma düzeyi",
                        out_vol_row,
                        palette,
                    ),
                    Self::setting_item(
                        "Ses Çıkış Aygıtı",
                        "wpctl ile kullanılacak varsayılan PipeWire çıkış aygıtı veya node kimliği",
                        output_device,
                        palette,
                    ),
                    Self::setting_item(
                        "Mikrofon Giriş Düzeyi",
                        "Varsayılan kayıt seviyesi ve giriş hassasiyeti",
                        in_vol_slider,
                        palette,
                    ),
                    Self::setting_item(
                        "Tüm Sesleri Sessize Al",
                        "Tüm sistem ses çıkışını anında sustur",
                        mute_cb,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_bluetooth(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Bluetooth Aygıtları")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Kablosuz çevre birimlerini, ses aygıtlarını ve denetleyicileri yönetin")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let power_cb = Self::pill_switch(
            self.bluetooth_powered,
            Message::ToggleBluetoothPower(!self.bluetooth_powered),
            palette,
        );

        let mut devices_col = column![].spacing(Spacing::SM);
        if !self.bluetooth_powered {
            devices_col = devices_col.push(
                text("Bluetooth kapalı. Aygıtları görmek ve bağlanmak için Bluetooth'u açın.")
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
            );
        } else if self.bluetooth_devices.is_empty() {
            devices_col = devices_col.push(
                text("Eşleşmiş veya yakında herhangi bir Bluetooth aygıtı bulunamadı.")
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
            );
        } else {
            for dev in &self.bluetooth_devices {
                let is_conn = dev.is_connected;
                let path = dev.path.clone();

                let action_btn = button(
                    text(if is_conn { "Bağlantıyı Kes" } else { "Bağlan" })
                        .size(Typography::BODY_SM)
                        .color(if is_conn { palette.danger.to_iced() } else { Color::WHITE }),
                )
                .padding([Spacing::XS, Spacing::MD])
                .style(move |_, status| {
                    if is_conn {
                        secondary_button_style(palette, status)
                    } else {
                        primary_button_style(palette, status)
                    }
                })
                .on_press(Message::ToggleBluetoothDevice(path, is_conn));

                let card = container(
                    row![
                        column![
                            text(dev.name.clone())
                                .size(Typography::BODY)
                                .color(palette.text_primary.to_iced()),
                            text(dev.address.clone())
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        action_btn,
                    ]
                    .align_y(Alignment::Center),
                )
                .padding(Spacing::MD)
                .width(Length::Fill)
                .style(move |_| card_style(palette));

                devices_col = devices_col.push(card);
            }
        }

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::setting_row(
                "Bluetooth Gücü",
                "Kablosuz Bluetooth adaptörünü etkinleştirin veya kapatın",
                power_cb,
                palette,
            ),
            vertical_space().height(Spacing::XS),
            text("AYGITLAR")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
            devices_col,
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_power(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Güç ve Pil")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Enerji profillerini, boşta kalma sürelerini ve ekran uyku zaman aşımını yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let profile_control = row![
            Self::segment_button(
                "Güç Tasarrufu",
                self.power.power_profile == "power-saver",
                Message::SetPowerProfile("power-saver".into()),
                palette,
            ),
            Self::segment_button(
                "Dengeli",
                self.power.power_profile == "balanced",
                Message::SetPowerProfile("balanced".into()),
                palette,
            ),
            Self::segment_button(
                "Performans",
                self.power.power_profile == "performance",
                Message::SetPowerProfile("performance".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let sleep_control = row![
            Self::segment_button(
                "5 dk",
                self.power.screen_sleep_mins == 5,
                Message::SetScreenSleep(5),
                palette,
            ),
            Self::segment_button(
                "15 dk",
                self.power.screen_sleep_mins == 15,
                Message::SetScreenSleep(15),
                palette,
            ),
            Self::segment_button(
                "30 dk",
                self.power.screen_sleep_mins == 30,
                Message::SetScreenSleep(30),
                palette,
            ),
            Self::segment_button(
                "Hiçbir zaman",
                self.power.screen_sleep_mins == 0,
                Message::SetScreenSleep(0),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Enerji Profili",
                        "Pil ömrü veya performans için işlemci hızını ve ısıl sınırları yönetin",
                        profile_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Ekranı Kapatma Süresi",
                        "Klavye veya fare etkinliği algılanmadığında ekranı otomatik olarak kapatın",
                        sleep_control,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_notifications(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Bildirim Servisi")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Bildirim sürelerini ve otomatik kapatma kurallarını yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let timeout_control = row![
            Self::segment_button(
                "3s",
                self.notify.default_timeout_ms == 3000,
                Message::SetNotifyTimeout(3000),
                palette,
            ),
            Self::segment_button(
                "5 sn (Varsayılan)",
                self.notify.default_timeout_ms == 5000,
                Message::SetNotifyTimeout(5000),
                palette,
            ),
            Self::segment_button(
                "8s",
                self.notify.default_timeout_ms == 8000,
                Message::SetNotifyTimeout(8000),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Bildirim Kapatma Süresi",
                        "Standart bildirim kartlarının ekranda kalacağı süre",
                        timeout_control,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_compositor(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Pencere Yöneticisi ve Döşeme")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Master-Stack dinamik döşeme düzenini, pencere aralıklarını ve oranlarını yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let gap_control = row![
            Self::segment_button(
                "0px",
                self.compositor.gap == 0,
                Message::SetCompositorGap(0),
                palette,
            ),
            Self::segment_button(
                "12px (Standard)",
                self.compositor.gap == 12,
                Message::SetCompositorGap(12),
                palette,
            ),
            Self::segment_button(
                "20px",
                self.compositor.gap == 20,
                Message::SetCompositorGap(20),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let layout_control = row![
            Self::segment_button(
                "Floating (Serbest)",
                self.compositor.layout_mode == corten_config::LayoutMode::Floating,
                Message::SetCompositorLayoutMode(corten_config::LayoutMode::Floating),
                palette,
            ),
            Self::segment_button(
                "Master-Stack",
                self.compositor.layout_mode == corten_config::LayoutMode::MasterStack,
                Message::SetCompositorLayoutMode(corten_config::LayoutMode::MasterStack),
                palette,
            ),
            Self::segment_button(
                "Dwindle (Spiral)",
                self.compositor.layout_mode == corten_config::LayoutMode::Dwindle,
                Message::SetCompositorLayoutMode(corten_config::LayoutMode::Dwindle),
                palette,
            ),
            Self::segment_button(
                "BSP (Ağaç)",
                self.compositor.layout_mode == corten_config::LayoutMode::Bsp,
                Message::SetCompositorLayoutMode(corten_config::LayoutMode::Bsp),
                palette,
            ),
            Self::segment_button(
                "Monocle (Tekli)",
                self.compositor.layout_mode == corten_config::LayoutMode::Monocle,
                Message::SetCompositorLayoutMode(corten_config::LayoutMode::Monocle),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Varsayılan Döşeme Algoritması",
                        "Yeni pencerelerin ekrana yerleştirilme düzeni (Super+Tab ile anlık değiştirilebilir)",
                        layout_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Döşeme Aralıkları (Kenar Boşlukları)",
                        "Döşeli pencereler arasındaki iç ve dış piksel aralığı",
                        gap_control,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_shortcuts(&self, palette: Palette) -> Element<'_, Message> {
        let heading = row![
            column![
                text("Klavye Kısayolları")
                    .size(Typography::TITLE)
                    .color(palette.text_primary.to_iced()),
                text("Kısayol kombinasyonlarını düzenleyin (ör. Super+Return, Super+E) ve kaydedin")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::XXS),
            horizontal_space(),
            button(
                text("Varsayılanlara Sıfırla")
                    .size(Typography::CAPTION)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([6, 12])
            .on_press(Message::ResetShortcutsToDefault)
            .style(move |_, status| {
                corten_theme::iced_styles::secondary_button_style(palette, status)
            }),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill);

        let shortcut_item =
            |label: &str, action_key: &str, chord: &str| -> Element<'_, Message> {
                let key_owned = action_key.to_string();
                let editor = text_input("Super+...", chord)
                    .on_input(move |val| Message::UpdateShortcut(key_owned.clone(), val))
                    .padding([4, 8])
                    .size(Typography::BODY_SM)
                    .width(Length::Fixed(140.0));

                container(
                    row![
                        text(label.to_string())
                            .size(Typography::BODY)
                            .color(palette.text_primary.to_iced())
                            .width(Length::Fill),
                        editor,
                    ]
                    .spacing(Spacing::MD)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
                )
                .padding([Spacing::SM, Spacing::MD])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(palette.surface.to_iced().into()),
                    border: Border {
                        color: palette.border.to_iced(),
                        width: 1.0,
                        radius: border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                })
                .into()
            };

        let shortcuts_list = column![
            shortcut_item("Spotlight / Pano", "launcher", &self.compositor.keybindings.launcher),
            shortcut_item("Uygulama Paneli", "dashboard", &self.compositor.keybindings.dashboard),
            shortcut_item("Terminali Aç", "terminal", &self.compositor.keybindings.terminal),
            shortcut_item("Dosya Yöneticisini Aç", "file_manager", &self.compositor.keybindings.file_manager),
            shortcut_item("Görev Yöneticisini Aç", "task_manager", &self.compositor.keybindings.task_manager),
            shortcut_item("Ayarları Aç", "settings", &self.compositor.keybindings.settings),
            shortcut_item(
                "Aktif Pencereyi Kapat",
                "close_window",
                &self.compositor.keybindings.close_window
            ),
            shortcut_item(
                "Sonraki Pencereye Odaklan",
                "focus_next",
                &self.compositor.keybindings.focus_next
            ),
            shortcut_item(
                "Önceki Pencereye Odaklan",
                "focus_previous",
                &self.compositor.keybindings.focus_previous
            ),
            shortcut_item(
                "Master Alanını Küçült",
                "decrease_master",
                &self.compositor.keybindings.decrease_master
            ),
            shortcut_item(
                "Master Alanını Büyüt",
                "increase_master",
                &self.compositor.keybindings.increase_master
            ),
            shortcut_item(
                "Pencereyi Sonraki Ekrana Taşı",
                "move_next_output",
                &self.compositor.keybindings.move_next_output
            ),
            shortcut_item(
                "Pencereyi Önceki Ekrana Taşı",
                "move_previous_output",
                &self.compositor.keybindings.move_previous_output
            ),
            shortcut_item("Ekran Görüntüsü", "screenshot", &self.compositor.keybindings.screenshot),
            shortcut_item("Tam Ekran", "toggle_fullscreen", &self.compositor.keybindings.toggle_fullscreen),
            shortcut_item("Döşeme Düzenini Değiştir", "cycle_layout", &self.compositor.keybindings.cycle_layout),
            shortcut_item("Yüzen Pencere Modu", "toggle_floating", &self.compositor.keybindings.toggle_floating),
            shortcut_item("Ana Pencereyle Yer Değiştir", "swap_master", &self.compositor.keybindings.swap_master),
            shortcut_item("Sonraki Pencereyle Yer Değiştir", "swap_next", &self.compositor.keybindings.swap_next),
            shortcut_item("Önceki Pencereyle Yer Değiştir", "swap_previous", &self.compositor.keybindings.swap_previous),
            shortcut_item("Pencereyi Küçült", "minimize", &self.compositor.keybindings.minimize),
            shortcut_item("Ekranı Kilitle", "lock_screen", &self.compositor.keybindings.lock_screen),
            shortcut_item("Çalışma Alanı 1", "workspace_1", &self.compositor.keybindings.workspace_1),
            shortcut_item("Çalışma Alanı 2", "workspace_2", &self.compositor.keybindings.workspace_2),
            shortcut_item("Çalışma Alanı 3", "workspace_3", &self.compositor.keybindings.workspace_3),
            shortcut_item("Çalışma Alanı 4", "workspace_4", &self.compositor.keybindings.workspace_4),
            shortcut_item("Çalışma Alanı 5", "workspace_5", &self.compositor.keybindings.workspace_5),
            shortcut_item("Çalışma Alanı 6", "workspace_6", &self.compositor.keybindings.workspace_6),
            shortcut_item("Çalışma Alanı 7", "workspace_7", &self.compositor.keybindings.workspace_7),
            shortcut_item("Çalışma Alanı 8", "workspace_8", &self.compositor.keybindings.workspace_8),
            shortcut_item("Çalışma Alanı 9", "workspace_9", &self.compositor.keybindings.workspace_9),
            shortcut_item("Pencereyi Çalışma Alanı 1'e Taşı", "move_to_workspace_1", &self.compositor.keybindings.move_to_workspace_1),
            shortcut_item("Pencereyi Çalışma Alanı 2'ye Taşı", "move_to_workspace_2", &self.compositor.keybindings.move_to_workspace_2),
            shortcut_item("Pencereyi Çalışma Alanı 3'e Taşı", "move_to_workspace_3", &self.compositor.keybindings.move_to_workspace_3),
            shortcut_item("Pencereyi Çalışma Alanı 4'e Taşı", "move_to_workspace_4", &self.compositor.keybindings.move_to_workspace_4),
            shortcut_item("Pencereyi Çalışma Alanı 5'e Taşı", "move_to_workspace_5", &self.compositor.keybindings.move_to_workspace_5),
            shortcut_item("Pencereyi Çalışma Alanı 6'ya Taşı", "move_to_workspace_6", &self.compositor.keybindings.move_to_workspace_6),
            shortcut_item("Pencereyi Çalışma Alanı 7'ye Taşı", "move_to_workspace_7", &self.compositor.keybindings.move_to_workspace_7),
            shortcut_item("Pencereyi Çalışma Alanı 8'e Taşı", "move_to_workspace_8", &self.compositor.keybindings.move_to_workspace_8),
            shortcut_item("Pencereyi Çalışma Alanı 9'a Taşı", "move_to_workspace_9", &self.compositor.keybindings.move_to_workspace_9),
        ]
        .spacing(Spacing::XS);

        column![
            heading,
            vertical_space().height(Spacing::SM),
            shortcuts_list,
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_accessibility(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Erişilebilirlik ve Kapsayıcı Tasarım")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Yüksek karşıtlık, renk körlüğü filtreleri, metin ölçeklendirme ve ekran okuyucu entegrasyonu")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let hc_toggle = Self::pill_switch(
            self.accessibility.high_contrast,
            Message::ToggleHighContrast(!self.accessibility.high_contrast),
            palette,
        );

        let color_filters = row![
            Self::segment_button(
                "Yok",
                self.accessibility.color_filter == ColorFilterChoice::None,
                Message::SelectColorFilter(ColorFilterChoice::None),
                palette,
            ),
            Self::segment_button(
                "Protanopi",
                self.accessibility.color_filter == ColorFilterChoice::Protanopia,
                Message::SelectColorFilter(ColorFilterChoice::Protanopia),
                palette,
            ),
            Self::segment_button(
                "Döteranopi",
                self.accessibility.color_filter == ColorFilterChoice::Deuteranopia,
                Message::SelectColorFilter(ColorFilterChoice::Deuteranopia),
                palette,
            ),
            Self::segment_button(
                "Tritanopi",
                self.accessibility.color_filter == ColorFilterChoice::Tritanopia,
                Message::SelectColorFilter(ColorFilterChoice::Tritanopia),
                palette,
            ),
            Self::segment_button(
                "Monokrom",
                self.accessibility.color_filter == ColorFilterChoice::Monochrome,
                Message::SelectColorFilter(ColorFilterChoice::Monochrome),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let font_scales = row![
            Self::segment_button(
                "1.0x",
                (self.accessibility.font_scale - 1.0).abs() < 0.05,
                Message::ChangeFontScale(1.0),
                palette,
            ),
            Self::segment_button(
                "1.25x",
                (self.accessibility.font_scale - 1.25).abs() < 0.05,
                Message::ChangeFontScale(1.25),
                palette,
            ),
            Self::segment_button(
                "1.5x",
                (self.accessibility.font_scale - 1.5).abs() < 0.05,
                Message::ChangeFontScale(1.5),
                palette,
            ),
            Self::segment_button(
                "1.75x",
                (self.accessibility.font_scale - 1.75).abs() < 0.05,
                Message::ChangeFontScale(1.75),
                palette,
            ),
            Self::segment_button(
                "2.0x",
                (self.accessibility.font_scale - 2.0).abs() < 0.05,
                Message::ChangeFontScale(2.0),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let cursor_toggle = Self::pill_switch(
            self.accessibility.large_cursor,
            Message::ToggleLargeCursor(!self.accessibility.large_cursor),
            palette,
        );

        let focus_ring_toggle = Self::pill_switch(
            self.accessibility.high_vis_focus_ring,
            Message::ToggleHighVisFocusRing(!self.accessibility.high_vis_focus_ring),
            palette,
        );

        let sound_feedback_toggle = Self::pill_switch(
            self.accessibility.sound_feedback,
            Message::ToggleSoundFeedback(!self.accessibility.sound_feedback),
            palette,
        );

        let screen_reader_toggle = Self::pill_switch(
            self.accessibility.screen_reader,
            Message::ToggleScreenReader(!self.accessibility.screen_reader),
            palette,
        );

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Yüksek Karşıtlık Modu (WCAG AAA)",
                        "Arayüz öğelerinde saf siyah/beyaz ve yüksek kontrastlı renkler (>18:1) uygular",
                        hc_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Renk Körlüğü ve Görüş Filtresi",
                        "Renk algısı bozuklukları için Bretell/Machado renk matrisi dönüşümü uygular",
                        color_filters,
                        palette,
                    ),
                    Self::setting_item(
                        "Genel Metin Ölçekleme",
                        "Tüm sistem yazı tipleri için metin ölçek katsayısını ayarlar",
                        font_scales,
                        palette,
                    ),
                    Self::setting_item(
                        "Büyük İmleç (48px)",
                        "İmleç boyutunu varsayılan 24px'ten yüksek görünürlüklü 48px'e çıkarır",
                        cursor_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Yüksek Görünürlüklü Odak Çerçevesi (3px)",
                        "Aktif odaklı pencere etrafında belirgin 3px kalın neon çerçeve çizer",
                        focus_ring_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Erişilebilirlik Sesli Geri Bildirimi",
                        "Masaüstü değişimi, pencere odağı ve ekran görüntüsü alımında sesli geri bildirim çalar",
                        sound_feedback_toggle,
                        palette,
                    ),
                    Self::setting_item(
                        "Ekran Okuyucu (AT-SPI2 / Orca)",
                        "Görme engelliler için AT-SPI2 D-Bus erişilebilirlik altyapısını ve Orca desteğini etkinleştirir",
                        screen_reader_toggle,
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }

    fn view_about(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Corten Hakkında")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Modern, bütünleşik ve bellek güvenli Wayland masaüstü ortamı")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let banner = container(
            column![
                row![
                    text(Icon::SHIELD)
                        .font(LUCIDE_FONT)
                        .size(32)
                        .color(palette.primary.to_iced()),
                    column![
                        text("Corten OS Paketi")
                            .size(Typography::TITLE_SM)
                            .color(palette.text_primary.to_iced()),
                        text("v0.1.0 (Geliştirme Sürümü)")
                            .size(Typography::CAPTION)
                            .color(palette.success.to_iced()),
                    ]
                    .spacing(2),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
                text(
                    "Corten, katı Unix felsefesi ilkelerini izleyen saf Rust ile sıfırdan geliştirildi. \
                     Her bileşen güvenilir bir bağımsız araç olarak çalışırken kesintisiz ve \
                     yüksek performanslı bütünleşik bir kullanıcı deneyimi sunar.",
                )
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::MD),
        )
        .padding(Spacing::LG)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.surface.to_iced().into()),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            ..Default::default()
        });

        column![heading, vertical_space().height(Spacing::SM), banner]
            .spacing(Spacing::MD)
            .into()
    }

    fn view_footer(&self, palette: Palette) -> Element<'_, Message> {
        let status = if let Some(msg) = &self.status_message {
            text(msg.clone())
                .size(Typography::BODY_SM)
                .color(palette.primary.to_iced())
        } else {
            text("Değişiklikler kaydedildiğinde diske hemen eşitlenir")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced())
        };

        let save_btn = button(
            row![
                text(Icon::POWER).font(LUCIDE_FONT).size(15),
                text("Değişiklikleri Kaydet"),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
        )
        .padding([Spacing::SM + 2.0, Spacing::LG])
        .on_press(Message::SaveSettings)
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.primary_hover.to_iced(),
                _ => palette.primary.to_iced(),
            };
            button::Style {
                background: Some(bg.into()),
                text_color: Color::WHITE,
                border: Border {
                    radius: border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                shadow: Shadow::default(),
            }
        });

        container(
            row![status, horizontal_space(), save_btn]
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .padding([Spacing::SM, 0.0])
        .into()
    }

    fn view_ai(&self, palette: Palette) -> Element<'_, Message> {
        let heading = column![
            text("Yapay Zeka (Corten AI)")
                .size(Typography::TITLE)
                .color(palette.text_primary.to_iced()),
            text("Yerel yapay zeka ajanını, model profillerini ve GPU/RAM kaynak kullanımını yapılandırın")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        let ai_enabled_cb = Self::pill_switch(
            self.ai.enabled,
            Message::ToggleAiEnabled(!self.ai.enabled),
            palette,
        );

        let model_control = row![
            Self::segment_button(
                "Eco (1.5B)",
                self.ai.model_tier == "eco",
                Message::SetAiModelTier("eco".into()),
                palette,
            ),
            Self::segment_button(
                "Dengeli (3B)",
                self.ai.model_tier == "balanced",
                Message::SetAiModelTier("balanced".into()),
                palette,
            ),
            Self::segment_button(
                "Pro (7B)",
                self.ai.model_tier == "pro",
                Message::SetAiModelTier("pro".into()),
                palette,
            ),
        ]
        .spacing(Spacing::XS);

        let game_gpu_cb = Self::pill_switch(
            self.ai.auto_gpu_release_on_game,
            Message::ToggleAiGameGpuRelease(!self.ai.auto_gpu_release_on_game),
            palette,
        );

        column![
            heading,
            vertical_space().height(Spacing::SM),
            Self::settings_group(
                vec![
                    Self::setting_item(
                        "Corten AI Sistem Asistanı",
                        "Wayland HUD (Super+Alt+Space) ve Başlatıcı doğal dil sistem komutlarını etkinleştir",
                        ai_enabled_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Yerel Model Profili & Kaynak Tüketimi",
                        "Sistem donanımına göre optimize edilmiş yerel model kademesi (AURA-CORE / GGUF)",
                        model_control,
                        palette,
                    ),
                    Self::setting_item(
                        "Oyun Modu GPU Koruması",
                        "Tam ekran bir oyun veya 3D render açıldığında model VRAM'i boşaltılarak maksimum FPS sağlanır",
                        game_gpu_cb,
                        palette,
                    ),
                    Self::setting_item(
                        "Yerel Motor Adresi",
                        "AURA-CORE veya Ollama API soket bağlantı noktası: http://127.0.0.1:8000",
                        text("127.0.0.1:8000 (Aktif)").size(Typography::BODY_SM).color(palette.success.to_iced()),
                        palette,
                    ),
                ],
                palette,
            ),
        ]
        .spacing(Spacing::MD)
        .into()
    }
}

fn next_category_index(current: usize, direction: i32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if direction > 0 {
        (current + 1) % count
    } else if current == 0 {
        count - 1
    } else {
        current - 1
    }
}

#[cfg(test)]
mod tests {
    use super::{next_category_index, CortenSettingsApp};

    #[test]
    fn keyboard_category_navigation_wraps() {
        assert_eq!(next_category_index(11, 1, 12), 0);
        assert_eq!(next_category_index(0, -1, 13), 12);
        assert_eq!(next_category_index(4, 1, 13), 5);
    }

    #[test]
    fn settings_exposes_all_categories() {
        assert_eq!(CortenSettingsApp::categories().len(), 15);
    }

    #[test]
    fn test_display_offset_and_arrangement_update() {
        use super::Message;
        let mut app = CortenSettingsApp::default();
        let _ = app.update(Message::SetDisplayArrangement("extend-left".into()));
        assert_eq!(app.displays.arrangement, "extend-left");

        let _ = app.update(Message::SetDisplayOffsetY(240));
        assert_eq!(app.displays.offset_y, 240);

        let _ = app.update(Message::SetDisplayOffsetY(-120));
        assert_eq!(app.displays.offset_y, -120);
    }
}
