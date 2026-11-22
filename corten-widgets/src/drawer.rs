use std::borrow::Cow;
use std::time::Duration;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS,
    NOTO_SANS_BYTES,
};
use iced::widget::{button, container, horizontal_space, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

use crate::config::WidgetsConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerCategory {
    All,
    System,
    TimeWeather,
    Tools,
}

pub struct DrawerFlags {
    pub config_manager: ConfigManager<WidgetsConfig>,
}

#[derive(Debug, Clone)]
pub enum DrawerMessage {
    CategorySelected(DrawerCategory),
    ToggleWidget(&'static str),
    CloseDrawer,
    Tick,
    #[allow(dead_code)]
    LayerAction(LayershellCustomActions),
}

impl TryInto<LayershellCustomActions> for DrawerMessage {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            DrawerMessage::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

pub struct CortenWidgetDrawerApp {
    config_manager: ConfigManager<WidgetsConfig>,
    config: WidgetsConfig,
    active_category: DrawerCategory,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
}

struct WidgetCatalogEntry {
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    icon: char,
    category: DrawerCategory,
    is_enabled: bool,
}

impl CortenWidgetDrawerApp {
    pub fn run_drawer(config_manager: ConfigManager<WidgetsConfig>) -> Result<(), Box<dyn std::error::Error>> {
        let flags = DrawerFlags { config_manager };

        CortenWidgetDrawerApp::run(Settings {
            id: Some("corten-widget-drawer".into()),
            flags,
            fonts: vec![
                Cow::Borrowed(LUCIDE_FONT_BYTES),
                Cow::Borrowed(NOTO_SANS_BYTES),
            ],
            layer_settings: LayerShellSettings {
                anchor: Anchor::Bottom | Anchor::Left | Anchor::Right,
                layer: Layer::Top, // Appears above open windows
                margin: (0, 32, 74, 32), // Floats gracefully above the panel dock
                exclusive_zone: 0,
                size: Some((0, 360)),
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

    fn save_config_changes(&mut self) {
        let _ = self.config_manager.update(self.config.clone());
    }

    fn category_pill<'a>(
        &self,
        label: &'static str,
        cat: DrawerCategory,
        palette: Palette,
    ) -> Element<'a, DrawerMessage> {
        let is_active = self.active_category == cat;
        button(
            text(label)
                .size(11.5)
                .font(iced::Font {
                    weight: if is_active {
                        iced::font::Weight::Bold
                    } else {
                        iced::font::Weight::Normal
                    },
                    ..Default::default()
                })
                .color(if is_active {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .padding([4, 12])
        .style(move |_, status| {
            let bg = if is_active {
                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.16)
            } else if status == button::Status::Hovered {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07)
            } else {
                iced::Color::from_rgba(0.0, 0.0, 0.0, 0.20)
            };
            let border_color = if is_active {
                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.50)
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06)
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: border_color,
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .on_press(DrawerMessage::CategorySelected(cat))
        .into()
    }

    fn build_drawer_card<'a>(
        &self,
        entry: &WidgetCatalogEntry,
        palette: Palette,
    ) -> Element<'a, DrawerMessage> {
        let is_enabled = entry.is_enabled;
        let id = entry.id;

        let toggle_btn = button(
            row![
                text(if is_enabled { "✓ Aktif" } else { "+ Ekle" })
                    .size(11.0)
                    .font(corten_theme::NOTO_SANS_MEDIUM)
                    .color(if is_enabled {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            ]
            .align_y(Alignment::Center),
        )
        .padding([5, 12])
        .style(move |_, status| {
            let bg = if is_enabled {
                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.20)
            } else if status == button::Status::Hovered {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
            } else {
                iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25)
            };
            let border_color = if is_enabled {
                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.50)
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: border_color,
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .on_press(DrawerMessage::ToggleWidget(id));

        let card_header = row![
            container(
                text(entry.icon)
                    .font(LUCIDE_FONT)
                    .size(16.0)
                    .color(if is_enabled {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            )
            .padding(6)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            }),
            text(entry.title)
                .size(12.5)
                .font(corten_theme::NOTO_SANS_BOLD)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let card_body = text(entry.desc)
            .size(11.0)
            .color(palette.text_muted.to_iced());

        container(
            iced::widget::column![
                card_header,
                card_body,
                horizontal_space(),
                row![horizontal_space(), toggle_btn].align_y(Alignment::Center),
            ]
            .spacing(8)
            .width(Length::Fixed(210.0))
            .height(Length::Fixed(140.0)),
        )
        .padding(12)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(if is_enabled {
                iced::Color::from_rgba(0.12, 0.13, 0.16, 0.85)
            } else {
                iced::Color::from_rgba(0.09, 0.10, 0.12, 0.60)
            })),
            border: iced::Border {
                color: if is_enabled {
                    iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.40)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07)
                },
                width: 1.0,
                radius: iced::border::Radius::from(12.0),
            },
            ..Default::default()
        })
        .into()
    }
}

impl Application for CortenWidgetDrawerApp {
    type Message = DrawerMessage;
    type Flags = DrawerFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = flags.config_manager.get().clone();

        let (active_patina, theme_mode) = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let m = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                (p, m)
            })
            .unwrap_or_else(|_| (PatinaVariant::Rust, ThemeMode::Dark));

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| DrawerMessage::Tick);

        (
            Self {
                config_manager: flags.config_manager,
                config,
                active_category: DrawerCategory::All,
                theme_mode,
                active_patina,
            },
            load_font_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-widget-drawer".to_string()
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
            DrawerMessage::CategorySelected(cat) => {
                self.active_category = cat;
                Task::none()
            }
            DrawerMessage::ToggleWidget(id) => {
                match id {
                    "sysinfo" => self.config.sysinfo_enabled = !self.config.sysinfo_enabled,
                    "network" => self.config.network_enabled = !self.config.network_enabled,
                    "clock" => self.config.clock_enabled = !self.config.clock_enabled,
                    "gpu" => self.config.gpu_enabled = !self.config.gpu_enabled,
                    "weather" => self.config.weather_enabled = !self.config.weather_enabled,
                    "weather_forecast" => self.config.weather_forecast_enabled = !self.config.weather_forecast_enabled,
                    "quick_actions" => self.config.quick_actions_enabled = !self.config.quick_actions_enabled,
                    "disk" => self.config.disk_enabled = !self.config.disk_enabled,
                    "pomodoro" => self.config.pomodoro_enabled = !self.config.pomodoro_enabled,
                    "music" => self.config.music_enabled = !self.config.music_enabled,
                    "scratchpad" => self.config.scratchpad_enabled = !self.config.scratchpad_enabled,
                    _ => {}
                }
                self.save_config_changes();
                Task::none()
            }
            DrawerMessage::CloseDrawer => {
                std::process::exit(0);
            }
            DrawerMessage::Tick => Task::none(),
            DrawerMessage::LayerAction(_) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        let entries = [
            WidgetCatalogEntry {
                id: "sysinfo",
                title: "CPU & RAM Monitörü",
                desc: "Anlık çekirdek yükü ve bellek doluluk göstergesi",
                icon: Icon::CPU,
                category: DrawerCategory::System,
                is_enabled: self.config.sysinfo_enabled,
            },
            WidgetCatalogEntry {
                id: "network",
                title: "Ağ & Bant Genişliği",
                desc: "İndirme/yükleme hızları ve aktif ağ arayüzü",
                icon: Icon::WIFI,
                category: DrawerCategory::System,
                is_enabled: self.config.network_enabled,
            },
            WidgetCatalogEntry {
                id: "clock",
                title: "Analog & Dünya Saati",
                desc: "Endüstriyel analog kadran ve dünya saatleri",
                icon: Icon::CLOCK,
                category: DrawerCategory::TimeWeather,
                is_enabled: self.config.clock_enabled,
            },
            WidgetCatalogEntry {
                id: "gpu",
                title: "GPU & Termal Telemetri",
                desc: "Ekran kartı yükü, çekirdek sıcaklığı ve VRAM",
                icon: Icon::GAUGE,
                category: DrawerCategory::System,
                is_enabled: self.config.gpu_enabled,
            },
            WidgetCatalogEntry {
                id: "weather_forecast",
                title: "Atmosferik Hava Durumu",
                desc: "Detaylı nem, rüzgar hızı ve 3 günlük tahmin",
                icon: Icon::CLOUD,
                category: DrawerCategory::TimeWeather,
                is_enabled: self.config.weather_forecast_enabled,
            },
            WidgetCatalogEntry {
                id: "quick_actions",
                title: "Hızlı Eylemler & Araçlar",
                desc: "Terminal, snipper ve sık kullanılan kısayollar",
                icon: Icon::TERMINAL,
                category: DrawerCategory::Tools,
                is_enabled: self.config.quick_actions_enabled,
            },
            WidgetCatalogEntry {
                id: "disk",
                title: "Depolama & Disk Sağlığı",
                desc: "Kök ve ev dizini depolama kullanımı ve I/O",
                icon: Icon::FOLDER,
                category: DrawerCategory::System,
                is_enabled: self.config.disk_enabled,
            },
            WidgetCatalogEntry {
                id: "pomodoro",
                title: "Odaklanma Zamanlayıcısı",
                desc: "25/5 dakikalık Pomodoro çalışma seansları",
                icon: Icon::COFFEE,
                category: DrawerCategory::TimeWeather,
                is_enabled: self.config.pomodoro_enabled,
            },
            WidgetCatalogEntry {
                id: "music",
                title: "Medya & Dalga Formu",
                desc: "MPRIS oynatıcı kontrolleri ve ses spektrumu",
                icon: Icon::MUSIC,
                category: DrawerCategory::Tools,
                is_enabled: self.config.music_enabled,
            },
            WidgetCatalogEntry {
                id: "scratchpad",
                title: "Hızlı Not Defteri",
                desc: "Masaüstünde anlık karalama ve görev notları",
                icon: Icon::CLIPBOARD,
                category: DrawerCategory::Tools,
                is_enabled: self.config.scratchpad_enabled,
            },
        ];

        let active_count = entries.iter().filter(|e| e.is_enabled).count();

        // 1. Header Row
        let header_title = row![
            text(Icon::APP_WINDOW)
                .font(LUCIDE_FONT)
                .size(18.0)
                .color(palette.primary.to_iced()),
            iced::widget::column![
                text("BİLEŞEN GALERİSİ")
                    .size(14.0)
                    .font(corten_theme::NOTO_SANS_BOLD)
                    .color(palette.text_primary.to_iced()),
                text("Masaüstünüze eklemek veya kaldırmak istediğiniz modülleri seçin")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(2),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let count_badge = container(
            text(format!("{} / {} Aktif", active_count, entries.len()))
                .size(11.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(palette.primary.to_iced()),
        )
        .padding([4, 10])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.primary.r, palette.primary.g, palette.primary.b, 0.15,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let close_btn = button(
            text(Icon::X)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(DrawerMessage::CloseDrawer);

        let header = row![header_title, horizontal_space(), count_badge, close_btn]
            .spacing(10)
            .align_y(Alignment::Center);

        // 2. Category Filter Pills
        let category_pills = row![
            self.category_pill("Tümü", DrawerCategory::All, palette),
            self.category_pill("Sistem & Donanım", DrawerCategory::System, palette),
            self.category_pill("Zaman & Hava", DrawerCategory::TimeWeather, palette),
            self.category_pill("Üretkenlik & Araçlar", DrawerCategory::Tools, palette),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        // 3. Horizontal Cards Deck
        let mut cards_row = row![].spacing(12).padding([4, 4]);

        for entry in entries.iter() {
            if self.active_category != DrawerCategory::All && entry.category != self.active_category {
                continue;
            }

            let card = self.build_drawer_card(entry, palette);
            cards_row = cards_row.push(card);
        }

        let scrollable_deck = scrollable(cards_row)
            .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::default()))
            .width(Length::Fill);

        let drawer_container = container(
            iced::widget::column![header, category_pills, scrollable_deck]
                .spacing(14)
                .width(Length::Fill),
        )
        .padding([18, 24])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.11, 0.95,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.40),
                width: 1.0,
                radius: iced::border::Radius::from(20.0),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.50),
                offset: iced::Vector::new(0.0, -8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        });

        drawer_container.into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        iced::time::every(Duration::from_millis(500)).map(|_| DrawerMessage::Tick)
    }
}
