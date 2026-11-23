use std::time::Duration;
use tracing::info;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_i18n::Language;
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES, Palette, PatinaVariant,
    Radius, ThemeMode,
};
use iced::widget::{
    button, container, horizontal_space, mouse_area, progress_bar, row, scrollable, text,
    text_input,
};
use iced::{Alignment, Element, Length, Subscription};
use iced_layershell::Application;
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_runtime::Task;

mod clock;
mod config;
mod disk;
mod drawer;
mod gpu;
mod music;
mod network;
mod pomodoro;
mod quick_actions;
mod radial_gauge;
mod scratchpad;
mod sysinfo;
mod visualizer;
mod weather;

use clock::ClockData;
use config::{WidgetPosition, WidgetsConfig};
use disk::{DiskMetrics, DiskMonitor};
use drawer::CortenWidgetDrawerApp;
use gpu::{GpuMetrics, GpuMonitor};
use music::{MusicService, MusicTrack, PlayerKind};
use network::{NetworkMetrics, NetworkMonitor};
use pomodoro::PomodoroTimer;
use quick_actions::QuickActions;
use radial_gauge::RadialGauge;
use scratchpad::ScratchpadService;
use sysinfo::{SysMetrics, SysMonitor};
use visualizer::WaveformVisualizer;
use weather::{WeatherData, WeatherService};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let config_manager = ConfigManager::<WidgetsConfig>::load_or_default("widgets")?;

    // If --drawer argument is passed, launch the Slide-in Bottom Widget Drawer
    if args.iter().any(|arg| arg == "--drawer" || arg == "-d") {
        info!("Opening Corten Widget Gallery Drawer (corten-widgets --drawer)...");
        return CortenWidgetDrawerApp::run_drawer(config_manager);
    }

    // If --preview argument is passed, launch in a normal desktop window for live preview on any compositor
    if args.iter().any(|arg| arg == "--preview" || arg == "-p") {
        info!("Opening Corten Widgets in windowed preview mode (corten-widgets --preview)...");
        let cm = config_manager;
        iced::application(
            "Corten Widgets — Live Preview",
            CortenWidgetsApp::update_preview,
            CortenWidgetsApp::view_preview,
        )
        .subscription(CortenWidgetsApp::subscription_preview)
        .window(iced::window::Settings {
            size: iced::Size::new(396.0, 530.0),
            resizable: true,
            ..Default::default()
        })
        .font(LUCIDE_FONT_BYTES)
        .font(NOTO_SANS_BYTES)
        .default_font(NOTO_SANS)
        .run_with(move || CortenWidgetsApp::new(AppFlags { config_manager: cm }))?;
        return Ok(());
    }

    info!("Starting Corten Desktop Modular Widgets Daemon (corten-widgets)...");

    let initial_config = config_manager.get().clone();

    let anchor = match initial_config.position {
        WidgetPosition::TopRight | WidgetPosition::BottomRight => {
            Anchor::Top | Anchor::Bottom | Anchor::Right
        }
        WidgetPosition::TopLeft | WidgetPosition::BottomLeft => {
            Anchor::Top | Anchor::Bottom | Anchor::Left
        }
    };

    let flags = AppFlags { config_manager };

    CortenWidgetsApp::run(Settings {
        id: Some("corten-widgets".into()),
        flags,
        fonts: vec![
            std::borrow::Cow::Borrowed(LUCIDE_FONT_BYTES),
            std::borrow::Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Bottom, // Desktop level: placed directly above wallpaper, beneath any open windows
            margin: initial_config.margin,
            exclusive_zone: 0,
            size: Some((376, 0)), // Spans full available vertical height cleanly along desktop side
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(13.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;
    Ok(())
}

pub struct AppFlags {
    pub config_manager: ConfigManager<WidgetsConfig>,
}

#[derive(Debug, Clone)]
pub enum Message {
    FastTick,
    SlowTick,
    PlayPause,
    NextTrack,
    PrevTrack,
    TrackUpdated(MusicTrack),
    WeatherUpdated(WeatherData),
    ToggleVisibility,
    NoteChanged(String),
    ToggleNotesEdit,
    SaveNote,
    PomodoroToggle,
    PomodoroReset,
    PomodoroSwitchMode,
    QuickActionLaunch(&'static str),
    OpenDrawer,
    ToggleEditMode,
    ToggleSidePosition,
    DisableWidget(&'static str),
    LayerAction(LayershellCustomActions),
}

impl TryInto<LayershellCustomActions> for Message {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            Message::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

pub struct CortenWidgetsApp {
    #[allow(dead_code)]
    config_manager: ConfigManager<WidgetsConfig>,
    config: WidgetsConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
    config_rx: Option<std::sync::mpsc::Receiver<WidgetsConfig>>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    current_track: MusicTrack,
    visualizer: WaveformVisualizer,
    sys_monitor: SysMonitor,
    sys_metrics: SysMetrics,
    network_monitor: NetworkMonitor,
    network_metrics: NetworkMetrics,
    gpu_metrics: GpuMetrics,
    telemetry_history: std::collections::VecDeque<f32>,
    disk_monitor: DiskMonitor,
    disk_metrics: DiskMetrics,
    pomodoro: PomodoroTimer,
    clock_data: ClockData,
    weather: WeatherData,
    weather_tick_counter: u32,
    note_content: String,
    editing_note: bool,
    is_visible: bool,
    edit_mode: bool,
    #[allow(dead_code)]
    language: Language,
}

impl CortenWidgetsApp {
    fn update_preview(&mut self, message: Message) -> Task<Message> {
        <Self as Application>::update(self, message)
    }

    fn view_preview(&self) -> Element<'_, Message> {
        let fg = container(<Self as Application>::view(self))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(16)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top);

        let wallpaper_path =
            std::path::PathBuf::from("assets/wallpapers/corten_dark_landscape.jpg");
        if wallpaper_path.exists() {
            let bg = iced::widget::image(iced::widget::image::Handle::from_path(wallpaper_path))
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover);
            iced::widget::stack![bg, fg].into()
        } else {
            fg.style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.07, 0.08, 0.10,
                ))),
                ..Default::default()
            })
            .into()
        }
    }

    fn subscription_preview(&self) -> Subscription<Message> {
        <Self as Application>::subscription(self)
    }

    fn layer_config_task(&self) -> Task<Message> {
        let anchor = match self.config.position {
            WidgetPosition::TopRight | WidgetPosition::BottomRight => {
                Anchor::Top | Anchor::Bottom | Anchor::Right
            }
            WidgetPosition::TopLeft | WidgetPosition::BottomLeft => {
                Anchor::Top | Anchor::Bottom | Anchor::Left
            }
        };

        Task::batch(vec![
            Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                anchor,
            ))),
            Task::done(Message::LayerAction(LayershellCustomActions::MarginChange(
                self.config.margin,
            ))),
        ])
    }
}

impl Application for CortenWidgetsApp {
    type Message = Message;
    type Flags = AppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = flags.config_manager.get().clone();
        let (watcher, rx) = match flags.config_manager.spawn_watcher() {
            Ok((w, r)) => (Some(w), Some(r)),
            Err(_) => (None, None),
        };

        let track = MusicTrack::default();
        let mut visualizer = WaveformVisualizer::new();
        visualizer.tick(track.is_playing);

        let mut sys_monitor = SysMonitor::new();
        let sys_metrics = sys_monitor.sample();

        let mut network_monitor = NetworkMonitor::new();
        let network_metrics = network_monitor.sample();

        let gpu_metrics = GpuMonitor::sample();
        let mut telemetry_history = std::collections::VecDeque::with_capacity(28);
        let base_wave = [
            18.0, 22.0, 26.0, 21.0, 17.0, 19.0, 24.0, 38.0, 44.0, 41.0, 43.0, 54.0, 62.0, 58.0,
            51.0, 55.0, 61.0, 56.0, 42.0, 28.0,
        ];
        for v in base_wave {
            telemetry_history.push_back(v);
        }

        let mut disk_monitor = DiskMonitor::new();
        let disk_metrics = disk_monitor.sample();

        let pomodoro = PomodoroTimer::new();
        let clock_data = ClockData::now();

        let note_content = ScratchpadService::load_note();
        let weather = WeatherService::load_cached_or_default();
        let (active_patina, theme_mode, language) =
            ConfigManager::<AppearanceConfig>::load_or_default("appearance")
                .map(|c| {
                    let p = PatinaVariant::from_code(&c.get().patina);
                    let m = match c.get().theme_mode {
                        corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                        corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                    };
                    let l = Language::from_code(&c.get().language);
                    (p, m, l)
                })
                .unwrap_or_else(|_| {
                    (
                        PatinaVariant::Rust,
                        ThemeMode::Dark,
                        Language::detect_system(),
                    )
                });

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::SlowTick);
        let init_track_task = Task::perform(
            MusicService::fetch_current_track_async(),
            Message::TrackUpdated,
        );
        let init_weather_task = Task::perform(
            WeatherService::fetch_weather_async(),
            Message::WeatherUpdated,
        );

        let init_task = Task::batch(vec![load_font_task, init_track_task, init_weather_task]);

        (
            Self {
                config_manager: flags.config_manager,
                config,
                _config_watcher: watcher,
                config_rx: rx,
                theme_mode,
                active_patina,
                current_track: track,
                visualizer,
                sys_monitor,
                sys_metrics,
                network_monitor,
                network_metrics,
                gpu_metrics,
                telemetry_history,
                disk_monitor,
                disk_metrics,
                pomodoro,
                clock_data,
                weather,
                weather_tick_counter: 0,
                note_content,
                editing_note: false,
                is_visible: true,
                edit_mode: false,
                language,
            },
            init_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-widgets".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: palette.text_primary.to_iced(),
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::FastTick => {
                // High-frequency tick for audio waveform animation
                self.visualizer.tick(self.current_track.is_playing);
                Task::none()
            }
            Message::SlowTick => {
                // Event-driven config sync: drain any pending watcher messages
                let mut needs_reconfig = false;
                if let Some(ref rx) = self.config_rx {
                    while let Ok(new_config) = rx.try_recv() {
                        if new_config != self.config {
                            self.config = new_config;
                            needs_reconfig = true;
                        }
                    }
                }

                self.sys_metrics = self.sys_monitor.sample();
                self.network_metrics = self.network_monitor.sample();
                self.gpu_metrics = GpuMonitor::sample();
                let phase = (self.weather_tick_counter as f32) * 0.68;
                let ripple = 28.0 + (phase.sin() * 14.0) + ((phase * 1.65 + 1.2).cos() * 11.0);
                let sample_val = (ripple
                    + self.sys_metrics.cpu_usage as f32 * 0.42
                    + self.gpu_metrics.gpu_util as f32 * 0.25)
                    .clamp(12.0, 92.0);
                if self.telemetry_history.len() >= 20 {
                    self.telemetry_history.pop_front();
                }
                self.telemetry_history.push_back(sample_val);
                self.disk_metrics = self.disk_monitor.sample();
                self.clock_data = ClockData::now();
                self.pomodoro.tick();

                if let Ok(app_cfg) =
                    ConfigManager::<AppearanceConfig>::load_or_default("appearance")
                {
                    self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    self.theme_mode = match app_cfg.get().theme_mode {
                        corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                        corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                    };
                }

                // Poll active MPRIS track status
                let fetch_track = Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                );

                // Periodic weather refresh (~every 5 minutes = 300 slow ticks)
                self.weather_tick_counter = (self.weather_tick_counter + 1) % 300;
                let fetch_weather = if self.weather_tick_counter == 0 {
                    Task::perform(
                        WeatherService::fetch_weather_async(),
                        Message::WeatherUpdated,
                    )
                } else {
                    Task::none()
                };

                let reconfig_task = if needs_reconfig {
                    self.layer_config_task()
                } else {
                    Task::none()
                };

                Task::batch(vec![fetch_track, fetch_weather, reconfig_task])
            }
            Message::PlayPause => {
                MusicService::play_pause();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::NextTrack => {
                MusicService::next();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::PrevTrack => {
                MusicService::previous();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::TrackUpdated(track) => {
                self.current_track = track;
                Task::none()
            }
            Message::WeatherUpdated(weather) => {
                self.weather = weather;
                Task::none()
            }
            Message::ToggleVisibility => {
                self.is_visible = !self.is_visible;
                Task::none()
            }
            Message::NoteChanged(val) => {
                self.note_content = val;
                Task::none()
            }
            Message::ToggleNotesEdit => {
                self.editing_note = !self.editing_note;
                if !self.editing_note {
                    ScratchpadService::save_note(&self.note_content);
                }
                Task::none()
            }
            Message::SaveNote => {
                self.editing_note = false;
                ScratchpadService::save_note(&self.note_content);
                Task::none()
            }
            Message::PomodoroToggle => {
                self.pomodoro.toggle_run();
                Task::none()
            }
            Message::PomodoroReset => {
                self.pomodoro.reset();
                Task::none()
            }
            Message::PomodoroSwitchMode => {
                self.pomodoro.switch_mode();
                Task::none()
            }
            Message::QuickActionLaunch(cmd) => {
                QuickActions::launch(cmd);
                Task::none()
            }
            Message::OpenDrawer => {
                let _ = std::process::Command::new("corten-widgets")
                    .arg("--drawer")
                    .spawn();
                Task::none()
            }
            Message::ToggleEditMode => {
                self.edit_mode = !self.edit_mode;
                Task::none()
            }
            Message::ToggleSidePosition => {
                self.config.position = match self.config.position {
                    WidgetPosition::TopRight | WidgetPosition::BottomRight => {
                        WidgetPosition::TopLeft
                    }
                    WidgetPosition::TopLeft | WidgetPosition::BottomLeft => {
                        WidgetPosition::TopRight
                    }
                };
                let _ = self.config_manager.update(self.config.clone());
                self.layer_config_task()
            }
            Message::DisableWidget(id) => {
                match id {
                    "clock" => self.config.clock_enabled = false,
                    "weather" => self.config.weather_enabled = false,
                    "sysinfo" => {
                        self.config.sysinfo_enabled = false;
                        self.config.gpu_enabled = false;
                    }
                    "music" => self.config.music_enabled = false,
                    "pomodoro" => self.config.pomodoro_enabled = false,
                    "weather_forecast" => self.config.weather_forecast_enabled = false,
                    "network" => self.config.network_enabled = false,
                    "disk" => self.config.disk_enabled = false,
                    "scratchpad" => self.config.scratchpad_enabled = false,
                    _ => {}
                }
                let _ = self.config_manager.update(self.config.clone());
                Task::none()
            }
            Message::LayerAction(_) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if !self.is_visible {
            return container(horizontal_space()).width(0.0).height(0.0).into();
        }

        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        let (_, secondary_patina) = self.active_patina.colors();

        let mut cards = iced::widget::column![]
            .spacing(corten_theme::Spacing::SM)
            .width(Length::Fixed(336.0));

        // Plasma-style Edit Mode toolbar (only visible while editing)
        if self.edit_mode {
            let is_right = matches!(
                self.config.position,
                WidgetPosition::TopRight | WidgetPosition::BottomRight
            );
            let side_btn = button(
                text(if is_right { "Sağ Kenar" } else { "Sol Kenar" })
                    .size(corten_theme::Typography::CAPTION)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([5, 10])
            .style(move |_, status| {
                let mut s = corten_theme::iced_styles::m3_tonal_button(palette, false, status);
                s.border.radius = iced::border::Radius::from(Radius::FULL);
                s
            })
            .on_press(Message::ToggleSidePosition);

            let add_btn = button(
                text("+ Galeri")
                    .size(corten_theme::Typography::CAPTION)
                    .color(palette.on_primary_container().to_iced()),
            )
            .padding([5, 10])
            .style(move |_, status| {
                let mut s = corten_theme::iced_styles::m3_tonal_button(palette, true, status);
                s.border.radius = iced::border::Radius::from(Radius::FULL);
                s
            })
            .on_press(Message::OpenDrawer);

            let done_btn = button(
                text("Bitti")
                    .size(corten_theme::Typography::CAPTION)
                    .font(corten_theme::NOTO_SANS_SEMIBOLD)
                    .color(palette.on_primary().to_iced()),
            )
            .padding([5, 12])
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: Some(iced::Background::Color(if hovered {
                        palette.primary_hover.to_iced()
                    } else {
                        palette.primary.to_iced()
                    })),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleEditMode);

            let edit_bar = container(
                row![side_btn, horizontal_space(), add_btn, done_btn]
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .width(Length::Fixed(336.0))
            .padding([8, 10])
            .style(move |_| corten_theme::iced_styles::m3_surface_card(palette, Radius::LG));

            cards = cards.push(edit_bar);
        }

        fn wrap_editable<'a>(
            card: Element<'a, Message>,
            widget_id: &'static str,
            width: f32,
            edit_mode: bool,
            palette: Palette,
        ) -> Element<'a, Message> {
            if !edit_mode {
                return card;
            }
            let remove_btn = button(
                container(
                    text(Icon::X)
                        .font(LUCIDE_FONT)
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                )
                .width(Length::Fixed(24.0))
                .height(Length::Fixed(24.0))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0))
            .padding(0)
            .style(move |_, status| corten_theme::iced_styles::m3_icon_button(palette, status))
            .on_press(Message::DisableWidget(widget_id));

            let overlay = container(remove_btn)
                .width(Length::Fixed(width))
                .padding(6)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top);

            iced::widget::stack![card, overlay].into()
        }

        let edit_mode = self.edit_mode;

        // 1. Top Bento Row: Digital Clock (164px) + Weather (164px)
        if self.config.clock_enabled && self.config.weather_enabled {
            cards = cards.push(
                row![
                    wrap_editable(
                        self.view_clock_half_card(palette),
                        "clock",
                        164.0,
                        edit_mode,
                        palette
                    ),
                    wrap_editable(
                        self.view_weather_half_card(palette),
                        "weather",
                        164.0,
                        edit_mode,
                        palette
                    ),
                ]
                .spacing(corten_theme::Spacing::XS)
                .width(Length::Fixed(336.0)),
            );
        } else if self.config.clock_enabled {
            cards = cards.push(wrap_editable(
                self.view_clock_card(palette),
                "clock",
                336.0,
                edit_mode,
                palette,
            ));
        } else if self.config.weather_enabled {
            cards = cards.push(wrap_editable(
                self.view_weather_card(palette),
                "weather",
                336.0,
                edit_mode,
                palette,
            ));
        }

        // 2. System Telemetry Card (336px, clickable to open corten-monitor)
        if self.config.sysinfo_enabled || self.config.gpu_enabled {
            cards = cards.push(wrap_editable(
                self.view_sysinfo_card(palette, secondary_patina),
                "sysinfo",
                336.0,
                edit_mode,
                palette,
            ));
        }

        // 3. Interactive Media Player Card (336px, real MPRIS controls, no fake track)
        if self.config.music_enabled {
            cards = cards.push(wrap_editable(
                self.view_media_focus_card(palette),
                "music",
                336.0,
                edit_mode,
                palette,
            ));
        }

        // 4. Standalone Pomodoro Card when explicitly enabled
        if self.config.pomodoro_enabled {
            cards = cards.push(wrap_editable(
                self.view_pomodoro_card(palette),
                "pomodoro",
                336.0,
                edit_mode,
                palette,
            ));
        }

        // Optional extra modules toggled via Widget Drawer (--drawer)
        if self.config.weather_forecast_enabled {
            cards = cards.push(wrap_editable(
                self.view_weather_card(palette),
                "weather_forecast",
                336.0,
                edit_mode,
                palette,
            ));
        }
        if self.config.network_enabled {
            cards = cards.push(wrap_editable(
                self.view_network_card(palette),
                "network",
                336.0,
                edit_mode,
                palette,
            ));
        }
        if self.config.disk_enabled {
            cards = cards.push(wrap_editable(
                self.view_disk_card(palette),
                "disk",
                336.0,
                edit_mode,
                palette,
            ));
        }
        if self.config.scratchpad_enabled {
            cards = cards.push(wrap_editable(
                self.view_scratchpad_card(palette),
                "scratchpad",
                336.0,
                edit_mode,
                palette,
            ));
        }

        // Subtle M3 tonal Edit Mode pill at bottom (hidden while already in Edit Mode)
        if !self.edit_mode {
            let edit_pill = button(
                row![
                    text(Icon::SETTINGS)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_muted.to_iced()),
                    text("Düzenle")
                        .size(corten_theme::Typography::CAPTION)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .style(move |_, status| {
                let mut bg = match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        palette.surface_container_high().to_iced()
                    }
                    _ => palette.surface_container_low().to_iced(),
                };
                bg.a = 0.88;
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleEditMode);

            cards = cards.push(row![horizontal_space(), edit_pill].align_y(Alignment::Center));
        }

        let scroll_container = scrollable(container(cards).padding(iced::Padding {
            top: 2.0,
            right: 4.0,
            bottom: 4.0,
            left: 2.0,
        }))
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(0.0)
                .scroller_width(0.0)
                .margin(0.0),
        ))
        .style(move |_, _| scrollable::Style {
            container: container::Style::default(),
            vertical_rail: scrollable::Rail {
                background: None,
                border: iced::Border::default(),
                scroller: scrollable::Scroller {
                    color: iced::Color::TRANSPARENT,
                    border: iced::Border::default(),
                },
            },
            horizontal_rail: scrollable::Rail {
                background: None,
                border: iced::Border::default(),
                scroller: scrollable::Scroller {
                    color: iced::Color::TRANSPARENT,
                    border: iced::Border::default(),
                },
            },
            gap: None,
        })
        .height(Length::Fill);

        container(scroll_container)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([8, 8])
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let sigusr1_sub = Subscription::run_with_id(
            "sigusr1-stream",
            futures::stream::unfold(
                None,
                |mut sig_opt: Option<tokio::signal::unix::Signal>| async move {
                    loop {
                        if sig_opt.is_none() {
                            match tokio::signal::unix::signal(
                                tokio::signal::unix::SignalKind::user_defined1(),
                            ) {
                                Ok(s) => sig_opt = Some(s),
                                Err(_) => {
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
                            }
                        }
                        if let Some(ref mut sig) = sig_opt {
                            sig.recv().await;
                            return Some((Message::ToggleVisibility, sig_opt));
                        }
                    }
                },
            ),
        );

        let slow_sub = if self.is_visible {
            iced::time::every(Duration::from_secs(1)).map(|_| Message::SlowTick)
        } else {
            Subscription::none()
        };
        let fast_sub = if self.is_visible && self.current_track.is_playing {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::FastTick)
        } else {
            Subscription::none()
        };

        Subscription::batch(vec![sigusr1_sub, slow_sub, fast_sub])
    }
}

// Card View Implementations for all 10 Modular Widgets
impl CortenWidgetsApp {
    fn forged_card<'a>(
        &self,
        content: Element<'a, Message>,
        palette: Palette,
    ) -> Element<'a, Message> {
        container(content)
            .width(Length::Fixed(336.0))
            .padding(corten_theme::Spacing::MD)
            .style(move |_| {
                let mut bg = palette.surface_container_low().to_iced();
                bg.a = 0.96;
                container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(corten_theme::Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.36),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 16.0,
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    fn format_turkish_date(dt: &chrono::DateTime<chrono::Local>) -> String {
        use chrono::Datelike;
        let month = match dt.month() {
            1 => "Ocak",
            2 => "Şubat",
            3 => "Mart",
            4 => "Nisan",
            5 => "Mayıs",
            6 => "Haziran",
            7 => "Temmuz",
            8 => "Ağustos",
            9 => "Eylül",
            10 => "Ekim",
            11 => "Kasım",
            12 => "Aralık",
            _ => "",
        };
        let weekday = match dt.weekday() {
            chrono::Weekday::Mon => "Pazartesi",
            chrono::Weekday::Tue => "Salı",
            chrono::Weekday::Wed => "Çarşamba",
            chrono::Weekday::Thu => "Perşembe",
            chrono::Weekday::Fri => "Cuma",
            chrono::Weekday::Sat => "Cumartesi",
            chrono::Weekday::Sun => "Pazar",
        };
        format!("{} {} {}", dt.day(), month, weekday)
    }

    fn card_header<'a>(
        &self,
        icon_char: char,
        title: &'static str,
        subtitle: String,
        palette: Palette,
    ) -> Element<'a, Message> {
        row![
            text(icon_char)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text(title)
                .size(11.5)
                .font(corten_theme::NOTO_SANS_BOLD)
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            text(subtitle)
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    fn forged_half_card<'a>(
        &self,
        content: Element<'a, Message>,
        palette: Palette,
    ) -> Element<'a, Message> {
        container(content)
            .width(Length::Fixed(164.0))
            .height(Length::Fixed(128.0))
            .padding(corten_theme::Spacing::MD)
            .style(move |_| {
                let mut bg = palette.surface_container_low().to_iced();
                bg.a = 0.94;
                container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(corten_theme::Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.34),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 14.0,
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    // 1a. Half-width M3 Digital Clock card (164x128)
    fn view_clock_half_card(&self, palette: Palette) -> Element<'_, Message> {
        let time_str = self.clock_data.local_time.format("%H:%M").to_string();
        let date_str = Self::format_turkish_date(&self.clock_data.local_time);

        let body = iced::widget::column![
            row![
                text(Icon::CLOCK)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                horizontal_space(),
            ],
            iced::widget::vertical_space(),
            text(time_str)
                .size(34.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(palette.text_primary.to_iced()),
            text(date_str)
                .size(corten_theme::Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(2)
        .width(Length::Fill)
        .height(Length::Fill);

        self.forged_half_card(body.into(), palette)
    }

    // 1b. Half-width M3 Weather card (164x128)
    fn view_weather_half_card(&self, palette: Palette) -> Element<'_, Message> {
        let weather_icon = if self.weather.condition.contains("Bulut") {
            Icon::CLOUD
        } else if self.weather.condition.contains("Gece") {
            Icon::MOON
        } else {
            Icon::SUN
        };

        let city_short = if self.weather.location.chars().count() > 14 {
            format!(
                "{}…",
                self.weather.location.chars().take(13).collect::<String>()
            )
        } else {
            self.weather.location.clone()
        };

        let body = iced::widget::column![
            row![
                text(city_short)
                    .size(corten_theme::Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
                horizontal_space(),
                text(weather_icon)
                    .font(LUCIDE_FONT)
                    .size(16.0)
                    .color(palette.primary.to_iced()),
            ]
            .align_y(Alignment::Center)
            .width(Length::Fill),
            iced::widget::vertical_space(),
            text(format!("{:.0}°", self.weather.temperature))
                .size(34.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(palette.text_primary.to_iced()),
            text(&self.weather.condition)
                .size(corten_theme::Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(2)
        .width(Length::Fill)
        .height(Length::Fill);

        self.forged_half_card(body.into(), palette)
    }

    // 1. Time & weather card (full-width fallback when only clock is enabled)
    fn view_clock_card(&self, palette: Palette) -> Element<'_, Message> {
        let time_str = self.clock_data.local_time.format("%H:%M").to_string();
        let date_str = Self::format_turkish_date(&self.clock_data.local_time);

        let analog_canvas = self.clock_data.view(palette);

        let digital_info = iced::widget::column![
            text(time_str)
                .size(35.0)
                .font(corten_theme::NOTO_SANS)
                .color(palette.text_primary.to_iced()),
            text(date_str)
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(2)
        .align_x(Alignment::Center);

        let weather_icon = if self.weather.condition.contains("Bulut") {
            Icon::CLOUD
        } else if self.weather.condition.contains("Gece") {
            Icon::MOON
        } else {
            Icon::SUN
        };

        let weather_col = iced::widget::column![
            text(weather_icon)
                .font(LUCIDE_FONT)
                .size(21.0)
                .color(palette.text_muted.to_iced()),
            text(format!("{:.0}°C", self.weather.temperature))
                .size(14.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(6)
        .align_x(Alignment::Center);

        let body = row![
            analog_canvas,
            horizontal_space(),
            digital_info,
            horizontal_space(),
            weather_col,
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        self.forged_card(body.into(), palette)
    }

    // 2. System telemetry card (CPU, RAM, GPU)
    fn view_sysinfo_card(
        &self,
        palette: Palette,
        _secondary_patina: corten_theme::Color,
    ) -> Element<'_, Message> {
        let cpu_gauge = RadialGauge::new(
            self.sys_metrics.cpu_usage as f32,
            "CPU",
            format!("{}%", self.sys_metrics.cpu_usage),
            palette.primary.to_iced(),
        );

        let ram_gauge = RadialGauge::new(
            self.sys_metrics.ram_pct as f32,
            "RAM",
            format!("{}%", self.sys_metrics.ram_pct),
            palette.primary.to_iced(),
        );

        let gauges_row = row![cpu_gauge.view(), horizontal_space(), ram_gauge.view()]
            .align_y(Alignment::Center)
            .padding([4, 16]);

        let gpu_pill = row![
            horizontal_space(),
            container(
                row![
                    text("GPU")
                        .size(corten_theme::Typography::CAPTION)
                        .color(palette.text_muted.to_iced()),
                    text("•")
                        .size(corten_theme::Typography::CAPTION)
                        .color(palette.primary.to_iced()),
                    text(format!("{}°C", self.gpu_metrics.temp_c))
                        .size(corten_theme::Typography::CAPTION)
                        .font(corten_theme::NOTO_SANS_SEMIBOLD)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([4, 10])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(
                    palette.surface_container_high().to_iced(),
                )),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            }),
            horizontal_space(),
        ]
        .align_y(Alignment::Center);

        let wave_canvas = iced::widget::canvas::Canvas::new(TelemetryWaveCanvas {
            samples: self.telemetry_history.iter().copied().collect(),
            accent: palette.primary.to_iced(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(44.0));

        let card = self.forged_card(
            iced::widget::column![gauges_row, gpu_pill, wave_canvas]
                .spacing(corten_theme::Spacing::SM)
                .into(),
            palette,
        );

        mouse_area(card)
            .on_press(Message::QuickActionLaunch("corten-monitor"))
            .into()
    }

    // 3. Interactive M3 Media Player Card (no fake track, real MPRIS controls)
    fn view_media_focus_card(&self, palette: Palette) -> Element<'_, Message> {
        let has_track = !self.current_track.title.trim().is_empty()
            && self.current_track.title != "No Media Playing";
        let title_str = if has_track {
            self.current_track.title.clone()
        } else {
            "Medya çalmıyor".to_string()
        };
        let subtitle_str = if has_track {
            let player_name = self.current_track.player_kind.display_name();
            if self.current_track.artist.trim().is_empty() {
                player_name.to_string()
            } else {
                format!("{} • {}", player_name, self.current_track.artist)
            }
        } else {
            "MPRIS oynatıcı bekleniyor".to_string()
        };

        let album_tile: Element<'_, Message> = if has_track
            && let Some(ref path) = self.current_track.cover_path
            && path.exists()
        {
            let cover_img = iced::widget::image(iced::widget::image::Handle::from_path(path))
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(48.0))
                .content_fit(iced::ContentFit::Cover);

            let badge = iced::widget::canvas::Canvas::new(PlayerLogoCanvas {
                kind: self.current_track.player_kind,
                is_full_tile: false,
                accent: palette.primary.to_iced(),
            })
            .width(Length::Fixed(18.0))
            .height(Length::Fixed(18.0));

            let badge_overlay = container(badge)
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(48.0))
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Bottom);

            container(iced::widget::stack![cover_img, badge_overlay])
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(48.0))
                .into()
        } else if has_track {
            iced::widget::canvas::Canvas::new(PlayerLogoCanvas {
                kind: self.current_track.player_kind,
                is_full_tile: true,
                accent: palette.primary.to_iced(),
            })
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .into()
        } else {
            container(
                text(Icon::MUSIC)
                    .font(LUCIDE_FONT)
                    .size(20.0)
                    .color(palette.text_muted.to_iced()),
            )
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(
                    palette.surface_container_high().to_iced(),
                )),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
        };

        let transport_btn = |icon_char: char, is_primary: bool, msg: Message| {
            let fg = if is_primary {
                palette.on_primary().to_iced()
            } else {
                palette.text_primary.to_iced()
            };
            let size = if is_primary { 34.0 } else { 28.0 };
            button(
                container(text(icon_char).font(LUCIDE_FONT).size(13.0).color(fg))
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .padding(0)
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                let bg = if is_primary {
                    if hovered {
                        palette.primary_hover.to_iced()
                    } else {
                        palette.primary.to_iced()
                    }
                } else if hovered {
                    palette.surface_container_highest().to_iced()
                } else {
                    palette.surface_container_high().to_iced()
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                }
            })
            .on_press(msg)
        };

        let controls = row![
            transport_btn(Icon::SKIP_BACK, false, Message::PrevTrack),
            transport_btn(
                if self.current_track.is_playing {
                    Icon::PAUSE
                } else {
                    Icon::PLAY
                },
                true,
                Message::PlayPause,
            ),
            transport_btn(Icon::SKIP_FORWARD, false, Message::NextTrack),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let top_row = row![
            album_tile,
            iced::widget::column![
                text(title_str)
                    .size(corten_theme::Typography::BODY_SM)
                    .font(corten_theme::NOTO_SANS_SEMIBOLD)
                    .color(palette.text_primary.to_iced()),
                text(subtitle_str)
                    .size(corten_theme::Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(2)
            .width(Length::Fill),
            controls,
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let mut col = iced::widget::column![top_row].spacing(10);

        if has_track {
            let media_bar = progress_bar(0.0..=1.0, self.current_track.progress())
                .height(4.0)
                .style(move |_| progress_bar::Style {
                    background: iced::Background::Color(
                        palette.surface_container_highest().to_iced(),
                    ),
                    bar: iced::Background::Color(palette.primary.to_iced()),
                    border: iced::Border {
                        radius: iced::border::Radius::from(2.0),
                        ..Default::default()
                    },
                });
            let time_row = row![
                text(self.current_track.formatted_position())
                    .size(10.0)
                    .color(palette.text_muted.to_iced()),
                horizontal_space(),
                text(self.current_track.formatted_length())
                    .size(10.0)
                    .color(palette.text_muted.to_iced()),
            ];
            col = col.push(media_bar).push(time_row);
        }

        self.forged_card(col.into(), palette)
    }

    // 3. Network Telemetry Card
    fn view_network_card(&self, palette: Palette) -> Element<'_, Message> {
        let status_text = format!("{} · Bağlı", self.network_metrics.interface);
        let header = self.card_header(Icon::WIFI, "AĞ TELEMETRİSİ", status_text, palette);

        let rx_formatted = NetworkMonitor::format_rate(self.network_metrics.rx_speed_kb);
        let tx_formatted = NetworkMonitor::format_rate(self.network_metrics.tx_speed_kb);

        let rx_box = container(
            iced::widget::column![
                row![
                    text(Icon::ARROW_DOWN)
                        .font(LUCIDE_FONT)
                        .size(10.0)
                        .color(palette.primary.to_iced()),
                    text("İNDİRME")
                        .size(9.5)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                text(rx_formatted)
                    .size(13.0)
                    .font(corten_theme::NOTO_SANS_BOLD)
                    .color(palette.primary.to_iced()),
            ]
            .spacing(2),
        )
        .padding([6, 12])
        .width(Length::FillPortion(1))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.0, 0.0, 0.0, 0.25,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let tx_box = container(
            iced::widget::column![
                row![
                    text(Icon::ARROW_UP)
                        .font(LUCIDE_FONT)
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                    text("YÜKLEME")
                        .size(9.5)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                text(tx_formatted)
                    .size(13.0)
                    .font(corten_theme::NOTO_SANS_BOLD)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(2),
        )
        .padding([6, 12])
        .width(Length::FillPortion(1))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.0, 0.0, 0.0, 0.25,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let content = iced::widget::column![header, row![rx_box, tx_box].spacing(8)].spacing(8);

        self.forged_card(content.into(), palette)
    }

    // 4. GPU & Thermal Telemetry Card
    #[allow(dead_code)]
    fn view_gpu_card(
        &self,
        palette: Palette,
        secondary_patina: corten_theme::Color,
    ) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::GAUGE,
            "GPU & TERMAL",
            format!("{}°C · Normal", self.gpu_metrics.temp_c),
            palette,
        );

        let gpu_row = row![
            text("GPU Yükü")
                .size(11.0)
                .color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(format!("%{}", self.gpu_metrics.gpu_util))
                .size(11.5)
                .font(corten_theme::NOTO_SANS_BOLD)
                .color(palette.primary.to_iced()),
        ];

        let gpu_bar = progress_bar(0.0..=100.0, self.gpu_metrics.gpu_util as f32)
            .height(5.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(palette.primary.to_iced()),
                border: iced::Border::default(),
            });

        let temp_row = row![
            text("Sıcaklık")
                .size(11.0)
                .color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(format!("{} °C", self.gpu_metrics.temp_c))
                .size(11.5)
                .font(corten_theme::NOTO_SANS_BOLD)
                .color(secondary_patina.to_iced()),
        ];

        let temp_bar = progress_bar(20.0..=100.0, self.gpu_metrics.temp_c as f32)
            .height(5.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(secondary_patina.to_iced()),
                border: iced::Border::default(),
            });

        let content = iced::widget::column![
            header,
            iced::widget::column![gpu_row, gpu_bar].spacing(3),
            iced::widget::column![temp_row, temp_bar].spacing(3),
        ]
        .spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 5. Atmospheric Weather Card
    fn view_weather_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::CLOUD,
            "ATMOSFER & HAVA",
            self.weather.location.clone(),
            palette,
        );

        let weather_icon = if self.weather.condition.contains("Bulut") {
            Icon::CLOUD
        } else if self.weather.condition.contains("Gece") {
            Icon::MOON
        } else {
            Icon::SUN
        };

        let main_row = row![
            text(weather_icon)
                .font(LUCIDE_FONT)
                .size(24.0)
                .color(palette.primary.to_iced()),
            iced::widget::column![
                text(format!("{:.0}°C", self.weather.temperature))
                    .size(20.0)
                    .font(corten_theme::NOTO_SANS_BOLD)
                    .color(palette.text_primary.to_iced()),
                text(&self.weather.condition)
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(1),
            horizontal_space(),
            iced::widget::column![
                text(format!("Nem: %{}", self.weather.humidity))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
                text(format!("Rüzgar: {:.0} km/s", self.weather.wind_speed))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
                text(format!("Basınç: {} hPa", self.weather.pressure_hpa))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
            ]
            .align_x(Alignment::End)
            .spacing(1),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // 3-Day Forecast projection row
        let mut forecast_row = row![].spacing(6).width(Length::Fill);
        for day in self.weather.forecast.iter().take(3) {
            let day_cell = container(
                iced::widget::column![
                    text(&day.day_name)
                        .size(9.0)
                        .color(palette.text_muted.to_iced()),
                    text(format!("{}° / {}°", day.max_temp, day.min_temp))
                        .size(10.0)
                        .font(corten_theme::NOTO_SANS_MEDIUM)
                        .color(palette.text_primary.to_iced()),
                ]
                .align_x(Alignment::Center)
                .spacing(2),
            )
            .padding([4, 6])
            .width(Length::FillPortion(1))
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.20,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            });

            forecast_row = forecast_row.push(day_cell);
        }

        let content = iced::widget::column![header, main_row, forecast_row].spacing(8);
        self.forged_card(content.into(), palette)
    }

    // 6. Quick Actions Card
    #[allow(dead_code)]
    fn view_quick_actions_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::TERMINAL,
            "HIZLI EYLEMLER",
            "Kısayollar".into(),
            palette,
        );

        let mut row1 = row![].spacing(6).width(Length::Fill);
        let mut row2 = row![].spacing(6).width(Length::Fill);

        for (idx, action) in QuickActions::ACTIONS.iter().enumerate() {
            let btn = button(
                row![
                    text(action.icon_char)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.primary.to_iced()),
                    text(action.label)
                        .size(10.5)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([5, 8])
            .width(Length::FillPortion(1))
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(0.0, 0.0, 0.0, 0.22),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: iced::border::Radius::from(6.0),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::QuickActionLaunch(action.command));

            if idx < 3 {
                row1 = row1.push(btn);
            } else {
                row2 = row2.push(btn);
            }
        }

        let content = iced::widget::column![header, row1, row2].spacing(6);
        self.forged_card(content.into(), palette)
    }

    // 7. Disk Health Card
    fn view_disk_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::FOLDER,
            "DEPOLAMA SAĞLIĞI",
            "NVMe / SSD".into(),
            palette,
        );

        let root_info = self.disk_metrics.root.as_ref().map(|d| {
            let pct = d.used_pct;
            let bar = progress_bar(0.0..=100.0, pct as f32)
                .height(4.0)
                .style(move |_| progress_bar::Style {
                    background: iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.35,
                    )),
                    bar: iced::Background::Color(palette.primary.to_iced()),
                    border: iced::Border::default(),
                });
            iced::widget::column![
                row![
                    text("Kök (/)")
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    text(format!(
                        "{:.1} / {:.0} GB (%{})",
                        d.used_gb, d.total_gb, pct
                    ))
                    .size(10.5)
                    .color(palette.text_primary.to_iced()),
                ],
                bar,
            ]
            .spacing(2)
        });

        let home_info = self.disk_metrics.home.as_ref().map(|d| {
            let pct = d.used_pct;
            let bar = progress_bar(0.0..=100.0, pct as f32)
                .height(4.0)
                .style(move |_| progress_bar::Style {
                    background: iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.35,
                    )),
                    bar: iced::Background::Color(iced::Color::from_rgba(0.3, 0.8, 0.5, 0.8)),
                    border: iced::Border::default(),
                });
            iced::widget::column![
                row![
                    text("Ev (/home)")
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    text(format!(
                        "{:.1} / {:.0} GB (%{})",
                        d.used_gb, d.total_gb, pct
                    ))
                    .size(10.5)
                    .color(palette.text_primary.to_iced()),
                ],
                bar,
            ]
            .spacing(2)
        });

        let mut content = iced::widget::column![header].spacing(6);
        if let Some(r) = root_info {
            content = content.push(r);
        }
        if let Some(h) = home_info {
            content = content.push(h);
        }

        self.forged_card(content.into(), palette)
    }

    // 8. Pomodoro Chrono Card
    #[allow(dead_code)]
    fn view_pomodoro_card(&self, palette: Palette) -> Element<'_, Message> {
        let mode_title = self.pomodoro.mode.title();
        let header = self.card_header(
            Icon::COFFEE,
            "ODAKLANMA KRONOMETRESİ",
            format!("{} Seans", self.pomodoro.completed_sessions),
            palette,
        );

        let time_text = text(self.pomodoro.formatted_time())
            .size(24.0)
            .font(corten_theme::NOTO_SANS_BOLD)
            .color(palette.text_primary.to_iced());

        let p_bar = progress_bar(0.0..=1.0, self.pomodoro.progress())
            .height(4.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(palette.primary.to_iced()),
                border: iced::Border::default(),
            });

        let play_btn = button(
            text(if self.pomodoro.is_running {
                "⏸ Durdur"
            } else {
                "▶ Başlat"
            })
            .size(10.5)
            .color(palette.primary.to_iced()),
        )
        .padding([4, 10])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroToggle);

        let reset_btn = button(
            text("↺ Sıfırla")
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroReset);

        let mode_btn = button(
            text(mode_title)
                .size(10.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroSwitchMode);

        let controls = row![mode_btn, horizontal_space(), reset_btn, play_btn]
            .spacing(6)
            .align_y(Alignment::Center);

        let content = iced::widget::column![
            header,
            row![
                time_text,
                horizontal_space(),
                text(mode_title).size(11.0).color(palette.primary.to_iced())
            ]
            .align_y(Alignment::Center),
            p_bar,
            controls,
        ]
        .spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 9. Media & Waveform Card
    #[allow(dead_code)]
    fn view_music_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = row![
            text(Icon::MUSIC)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text(if self.current_track.artist.is_empty() {
                self.current_track.title.clone()
            } else {
                format!(
                    "{} - {}",
                    self.current_track.title, self.current_track.artist
                )
            })
            .size(11.0)
            .color(palette.text_primary.to_iced()),
            horizontal_space(),
            button(text("⏮").size(10.0).color(palette.text_muted.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::PrevTrack),
            button(text("⏸").size(10.0).color(palette.primary.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::PlayPause),
            button(text("⏭").size(10.0).color(palette.text_muted.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::NextTrack),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let visualizer_view = self.visualizer.view(palette.primary.to_iced());
        let content = iced::widget::column![header, visualizer_view].spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 10. Scratchpad Sticky Note Card
    fn view_scratchpad_card(&self, palette: Palette) -> Element<'_, Message> {
        let edit_btn = button(
            text(if self.editing_note { "✓" } else { "✏" })
                .size(11.0)
                .color(palette.primary.to_iced()),
        )
        .padding([2, 6])
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleNotesEdit);

        let header = row![
            text(Icon::CLIPBOARD)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text("HIZLI NOTLAR")
                .size(11.5)
                .font(corten_theme::NOTO_SANS_BOLD)
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            edit_btn,
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let note_body: Element<'_, Message> = if self.editing_note {
            text_input("Notunuzu yazın...", &self.note_content)
                .on_input(Message::NoteChanged)
                .on_submit(Message::SaveNote)
                .size(11.0)
                .padding(4)
                .style(move |_, _| text_input::Style {
                    background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.5,
                        ),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    icon: iced::Color::TRANSPARENT,
                    placeholder: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.4),
                    value: palette.text_primary.to_iced(),
                    selection: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.3,
                    ),
                })
                .into()
        } else {
            let display_text = if self.note_content.trim().is_empty() {
                "Not eklemek için tıklayın..."
            } else {
                self.note_content.as_str()
            };
            mouse_area(text(display_text).size(11.0).color(
                if self.note_content.trim().is_empty() {
                    palette.text_muted.to_iced()
                } else {
                    palette.text_primary.to_iced()
                },
            ))
            .on_press(Message::ToggleNotesEdit)
            .into()
        };

        let content = iced::widget::column![header, note_body].spacing(6);
        self.forged_card(content.into(), palette)
    }
}

struct PlayerLogoCanvas {
    kind: PlayerKind,
    is_full_tile: bool,
    accent: iced::Color,
}

impl<Message> iced::widget::canvas::Program<Message> for PlayerLogoCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::Point;
        use iced::widget::canvas::{Frame, LineCap, Path, Stroke, gradient};

        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let center = Point::new(w * 0.5, h * 0.5);

        if self.is_full_tile {
            // Draw dark desert-dune album tile background
            let r = 10.0_f32;
            let tile_path = Path::new(|b| {
                b.move_to(Point::new(r, 0.0));
                b.line_to(Point::new(w - r, 0.0));
                b.arc(iced::widget::canvas::path::Arc {
                    center: Point::new(w - r, r),
                    radius: r,
                    start_angle: iced::Radians(-std::f32::consts::FRAC_PI_2),
                    end_angle: iced::Radians(0.0),
                });
                b.line_to(Point::new(w, h - r));
                b.arc(iced::widget::canvas::path::Arc {
                    center: Point::new(w - r, h - r),
                    radius: r,
                    start_angle: iced::Radians(0.0),
                    end_angle: iced::Radians(std::f32::consts::FRAC_PI_2),
                });
                b.line_to(Point::new(r, h));
                b.arc(iced::widget::canvas::path::Arc {
                    center: Point::new(r, h - r),
                    radius: r,
                    start_angle: iced::Radians(std::f32::consts::FRAC_PI_2),
                    end_angle: iced::Radians(std::f32::consts::PI),
                });
                b.line_to(Point::new(0.0, r));
                b.arc(iced::widget::canvas::path::Arc {
                    center: Point::new(r, r),
                    radius: r,
                    start_angle: iced::Radians(std::f32::consts::PI),
                    end_angle: iced::Radians(std::f32::consts::PI * 1.5),
                });
                b.close();
            });

            let bg_grad = gradient::Linear::new(Point::new(0.0, 0.0), Point::new(w, h))
                .add_stop(0.0, iced::Color::from_rgba(0.14, 0.16, 0.21, 0.95))
                .add_stop(0.55, iced::Color::from_rgba(0.08, 0.09, 0.12, 0.95))
                .add_stop(
                    1.0,
                    iced::Color::from_rgba(self.accent.r * 0.35, self.accent.g * 0.25, 0.08, 0.95),
                );
            frame.fill(&tile_path, bg_grad);

            // Stylized dune horizon line across bottom of album art
            let dune = Path::new(|b| {
                b.move_to(Point::new(2.0, h * 0.72));
                b.bezier_curve_to(
                    Point::new(w * 0.35, h * 0.56),
                    Point::new(w * 0.65, h * 0.84),
                    Point::new(w - 2.0, h * 0.66),
                );
            });
            frame.stroke(
                &dune,
                Stroke::default()
                    .with_color(iced::Color::from_rgba(
                        self.accent.r,
                        self.accent.g,
                        self.accent.b,
                        0.45,
                    ))
                    .with_width(1.4),
            );

            let _ = &tile_path;
        }

        let badge_r = if self.is_full_tile {
            w.min(h) * 0.28
        } else {
            (w.min(h) * 0.5) - 0.5
        };

        match self.kind {
            PlayerKind::Spotify => {
                let circle = Path::circle(center, badge_r);
                frame.fill(&circle, iced::Color::from_rgb(0.11, 0.73, 0.33)); // Spotify Green #1DB954

                for (idx, (y_off, span, stroke_w)) in [
                    (-0.26_f32, 0.54_f32, 0.13_f32),
                    (0.02_f32, 0.44_f32, 0.11_f32),
                    (0.28_f32, 0.33_f32, 0.10_f32),
                ]
                .into_iter()
                .enumerate()
                {
                    let _ = idx;
                    let y = center.y + badge_r * y_off;
                    let dx = badge_r * span;
                    let wave = Path::new(|b| {
                        b.move_to(Point::new(center.x - dx, y + badge_r * 0.05));
                        b.quadratic_curve_to(
                            Point::new(center.x, y - badge_r * 0.14),
                            Point::new(center.x + dx, y + badge_r * 0.09),
                        );
                    });
                    frame.stroke(
                        &wave,
                        Stroke::default()
                            .with_color(iced::Color::from_rgb(0.04, 0.05, 0.06))
                            .with_width((badge_r * stroke_w).max(1.0))
                            .with_line_cap(LineCap::Round),
                    );
                }
            }
            PlayerKind::YouTubeMusic => {
                let circle = Path::circle(center, badge_r);
                frame.fill(&circle, iced::Color::from_rgb(1.0, 0.0, 0.20)); // YouTube Music Red #FF0033

                let inner_ring = Path::circle(center, badge_r * 0.58);
                frame.stroke(
                    &inner_ring,
                    Stroke::default()
                        .with_color(iced::Color::WHITE)
                        .with_width((badge_r * 0.10).max(0.9)),
                );

                let tri_r = badge_r * 0.30;
                let tri = Path::new(|b| {
                    b.move_to(Point::new(center.x - tri_r * 0.45, center.y - tri_r * 0.65));
                    b.line_to(Point::new(center.x + tri_r * 0.75, center.y));
                    b.line_to(Point::new(center.x - tri_r * 0.45, center.y + tri_r * 0.65));
                    b.close();
                });
                frame.fill(&tri, iced::Color::WHITE);
            }
            PlayerKind::Brave => {
                let circle = Path::circle(center, badge_r);
                frame.fill(&circle, iced::Color::from_rgb(0.98, 0.33, 0.17)); // Brave Orange-Red #FB542B

                // Stylized Brave Shield crest
                let s = badge_r * 0.56;
                let shield = Path::new(|b| {
                    b.move_to(Point::new(center.x - s * 0.75, center.y - s * 0.65));
                    b.line_to(Point::new(center.x, center.y - s * 0.85));
                    b.line_to(Point::new(center.x + s * 0.75, center.y - s * 0.65));
                    b.line_to(Point::new(center.x + s * 0.55, center.y + s * 0.25));
                    b.line_to(Point::new(center.x, center.y + s * 0.90));
                    b.line_to(Point::new(center.x - s * 0.55, center.y + s * 0.25));
                    b.close();
                });
                frame.fill(&shield, iced::Color::WHITE);
            }
            PlayerKind::Generic => {
                let circle = Path::circle(center, badge_r);
                frame.fill(&circle, self.accent);
                let inner = Path::circle(center, badge_r * 0.35);
                frame.fill(&inner, iced::Color::from_rgb(0.06, 0.07, 0.09));
            }
        }

        vec![frame.into_geometry()]
    }
}

struct TelemetryWaveCanvas {
    samples: Vec<f32>,
    accent: iced::Color,
}

impl<Message> iced::widget::canvas::Program<Message> for TelemetryWaveCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::Point;
        use iced::widget::canvas::{Frame, LineCap, LineJoin, Path, Stroke, Style, gradient};

        let mut frame = Frame::new(renderer, bounds.size());
        if self.samples.len() < 2 || bounds.width <= 4.0 || bounds.height <= 4.0 {
            return vec![frame.into_geometry()];
        }

        // Dynamic min/max normalization so the wave always displays rich organic peaks & valleys
        let min_v = self.samples.iter().copied().fold(f32::INFINITY, f32::min);
        let max_v = self
            .samples
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let span = (max_v - min_v).max(16.0);

        let n = self.samples.len();
        let step_x = bounds.width / (n - 1) as f32;
        let top_pad = 6.0;
        let bot_pad = 6.0;
        let usable_h = (bounds.height - top_pad - bot_pad).max(8.0);

        let points: Vec<Point> = self
            .samples
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let norm = ((val - min_v) / span).clamp(0.08, 0.94);
                let x = i as f32 * step_x;
                let y = top_pad + usable_h * (1.0 - norm);
                Point::new(x, y)
            })
            .collect();

        // Smooth cubic curve through sample points
        let wave_path = Path::new(|builder| {
            builder.move_to(points[0]);
            for i in 0..(points.len() - 1) {
                let p0 = points[i];
                let p1 = points[i + 1];
                let mid_x = (p0.x + p1.x) * 0.5;
                builder.bezier_curve_to(Point::new(mid_x, p0.y), Point::new(mid_x, p1.y), p1);
            }
        });

        // Vertical Linear Gradient area fill under curve (fades smoothly to 0.0 alpha at bottom)
        let area_path = Path::new(|builder| {
            builder.move_to(Point::new(0.0, bounds.height));
            builder.line_to(points[0]);
            for i in 0..(points.len() - 1) {
                let p0 = points[i];
                let p1 = points[i + 1];
                let mid_x = (p0.x + p1.x) * 0.5;
                builder.bezier_curve_to(Point::new(mid_x, p0.y), Point::new(mid_x, p1.y), p1);
            }
            builder.line_to(Point::new(bounds.width, bounds.height));
            builder.close();
        });

        let fill_grad =
            gradient::Linear::new(Point::new(0.0, top_pad), Point::new(0.0, bounds.height))
                .add_stop(
                    0.0,
                    iced::Color::from_rgba(self.accent.r, self.accent.g, self.accent.b, 0.28),
                )
                .add_stop(
                    0.65,
                    iced::Color::from_rgba(self.accent.r, self.accent.g, self.accent.b, 0.08),
                )
                .add_stop(1.0, iced::Color::TRANSPARENT);

        frame.fill(&area_path, fill_grad);

        // Crisp glowing copper-amber gradient core line
        let bright_amber = iced::Color::from_rgba(
            (self.accent.r + 0.16).min(1.0),
            (self.accent.g + 0.22).min(1.0),
            (self.accent.b + 0.16).min(1.0),
            0.98,
        );
        let line_grad = gradient::Linear::new(Point::new(0.0, 0.0), Point::new(bounds.width, 0.0))
            .add_stop(0.0, self.accent)
            .add_stop(0.55, bright_amber)
            .add_stop(1.0, self.accent);

        frame.stroke(
            &wave_path,
            Stroke {
                style: Style::Gradient(iced::widget::canvas::Gradient::Linear(line_grad)),
                width: 2.0,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            },
        );

        vec![frame.into_geometry()]
    }
}
