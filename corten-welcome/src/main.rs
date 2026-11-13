mod hardware;

use std::borrow::Cow;
use std::fs;
use std::time::Duration;

use corten_config::{AppearanceConfig, ConfigManager, DisplaySettingsConfig, ThemeModeChoice};
use corten_i18n::Language;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES,
};
use iced::widget::{
    button, column, container, horizontal_space, image, row, text, vertical_space,
};
use iced::{
    border, event, keyboard, Alignment, Border, Color, Element, Event, Length, Subscription,
};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

const LOGO_PNG: &[u8] = include_bytes!("../../assets/logo.png");

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Corten Welcome Presentation (Hoş Geldiniz Sunumu)...");

    CortenWelcomeApp::run(Settings {
        id: Some("corten-welcome".into()),
        flags: (),
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
            Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            layer: Layer::Overlay,
            margin: (0, 0, 0, 0),
            exclusive_zone: -1,
            size: None,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slide {
    Welcome = 0,
    Aesthetics = 1,
    DockAndNavigation = 2,
    Shortcuts = 3,
    AuraAi = 4,
    Ready = 5,
}

impl Slide {
    pub const ALL: [Slide; 6] = [
        Slide::Welcome,
        Slide::Aesthetics,
        Slide::DockAndNavigation,
        Slide::Shortcuts,
        Slide::AuraAi,
        Slide::Ready,
    ];

    pub fn next(&self) -> Option<Slide> {
        match self {
            Slide::Welcome => Some(Slide::Aesthetics),
            Slide::Aesthetics => Some(Slide::DockAndNavigation),
            Slide::DockAndNavigation => Some(Slide::Shortcuts),
            Slide::Shortcuts => Some(Slide::AuraAi),
            Slide::AuraAi => Some(Slide::Ready),
            Slide::Ready => None,
        }
    }

    pub fn prev(&self) -> Option<Slide> {
        match self {
            Slide::Welcome => None,
            Slide::Aesthetics => Some(Slide::Welcome),
            Slide::DockAndNavigation => Some(Slide::Aesthetics),
            Slide::Shortcuts => Some(Slide::DockAndNavigation),
            Slide::AuraAi => Some(Slide::Shortcuts),
            Slide::Ready => Some(Slide::AuraAi),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    GoToSlide(Slide),
    NextSlide,
    PrevSlide,
    SelectPatina(PatinaVariant),
    SelectThemeMode(ThemeMode),
    ToggleNightLight(bool),
    ToggleDontShowAgain(bool),
    CompleteWelcome,
    Tick,
    CloseApp,
    KeyboardEvent(Event),
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

pub struct CortenWelcomeApp {
    slide: Slide,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    #[allow(dead_code)]
    language: Language,
    pub night_light: bool,
    pub dont_show_again: bool,
    anim_tick: f32,
    appearance_mgr: Option<ConfigManager<AppearanceConfig>>,
    display_mgr: Option<ConfigManager<DisplaySettingsConfig>>,
    pub system_info: hardware::SystemInfo,
}

impl Default for CortenWelcomeApp {
    fn default() -> Self {
        let appearance_mgr = ConfigManager::<AppearanceConfig>::load_or_default("appearance").ok();
        let display_mgr = ConfigManager::<DisplaySettingsConfig>::load_or_default("displays").ok();
        let theme_mode = appearance_mgr
            .as_ref()
            .map(|c| match c.get().theme_mode {
                ThemeModeChoice::Light => ThemeMode::Light,
                ThemeModeChoice::Dark => ThemeMode::Dark,
            })
            .unwrap_or(ThemeMode::Dark);

        let active_patina = appearance_mgr
            .as_ref()
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        let language = appearance_mgr
            .as_ref()
            .map(|c| Language::from_code(&c.get().language))
            .unwrap_or_else(Language::detect_system);

        let night_light = display_mgr
            .as_ref()
            .map(|d| d.get().night_light)
            .unwrap_or(false);

        let system_info = hardware::SystemInfo::detect();

        Self {
            slide: Slide::Welcome,
            theme_mode,
            active_patina,
            language,
            night_light,
            dont_show_again: true,
            anim_tick: 0.0,
            appearance_mgr,
            display_mgr,
            system_info,
        }
    }
}

impl CortenWelcomeApp {
    fn save_appearance(&mut self) {
        if let Some(ref mut mgr) = self.appearance_mgr {
            let mut cfg = mgr.get().clone();
            cfg.patina = self.active_patina.code().to_string();
            let (prim, _) = self.active_patina.colors();
            cfg.accent_color = format!(
                "#{:02X}{:02X}{:02X}",
                (prim.r * 255.0).round() as u8,
                (prim.g * 255.0).round() as u8,
                (prim.b * 255.0).round() as u8
            );
            cfg.theme_mode = match self.theme_mode {
                ThemeMode::Light => ThemeModeChoice::Light,
                ThemeMode::Dark => ThemeModeChoice::Dark,
            };
            let _ = mgr.update(cfg);
        }

        if let Some(ref mut mgr) = self.display_mgr {
            let mut cfg = mgr.get().clone();
            cfg.night_light = self.night_light;
            let _ = mgr.update(cfg);
        }
    }
}

impl Application for CortenWelcomeApp {
    type Message = Message;
    type Flags = ();
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(_flags: Self::Flags) -> (Self, Task<Self::Message>) {
        (Self::default(), Task::none())
    }

    fn namespace(&self) -> String {
        "corten-welcome".to_string()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            event::listen().map(Message::KeyboardEvent),
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.anim_tick = (self.anim_tick + 0.05) % (std::f32::consts::PI * 2.0);
            }
            Message::GoToSlide(slide) => {
                self.slide = slide;
            }
            Message::NextSlide => {
                if let Some(next) = self.slide.next() {
                    self.slide = next;
                }
            }
            Message::PrevSlide => {
                if let Some(prev) = self.slide.prev() {
                    self.slide = prev;
                }
            }
            Message::SelectPatina(patina) => {
                self.active_patina = patina;
                self.save_appearance();
            }
            Message::SelectThemeMode(mode) => {
                self.theme_mode = mode;
                self.save_appearance();
            }
            Message::ToggleNightLight(val) => {
                self.night_light = val;
                self.save_appearance();
            }
            Message::ToggleDontShowAgain(val) => {
                self.dont_show_again = val;
            }
            Message::CompleteWelcome => {
                if self.dont_show_again {
                    if let Some(dirs) = directories::BaseDirs::new() {
                        let flag_path = dirs.config_dir().join("corten").join(".welcome_completed");
                        if let Some(parent) = flag_path.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        let _ = fs::write(flag_path, b"completed\n");
                    }
                }
                self.save_appearance();
                // Direct exit immediately unblocks corten-session into Stage 2!
                std::process::exit(0);
            }
            Message::CloseApp => {
                std::process::exit(0);
            }
            Message::KeyboardEvent(Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                ..
            })) => {
                // Only use Left/Right arrow keys for slide stepping; never auto-exit on ArrowRight
                // and never bind Space/Enter globally so focused Iced buttons don't double-fire.
                match key.as_ref() {
                    keyboard::Key::Named(keyboard::key::Named::ArrowRight) => {
                        if let Some(next) = self.slide.next() {
                            self.slide = next;
                        }
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => {
                        if let Some(prev) = self.slide.prev() {
                            self.slide = prev;
                        }
                    }
                    _ => {}
                }
            }
            Message::KeyboardEvent(_) => {}
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        let pulse = (self.anim_tick.sin() * 0.5 + 0.5).clamp(0.0, 1.0);

        // Header with Brand, Slide Indicator Dots, and Skip/Start button
        let logo_image = image(image::Handle::from_bytes(LOGO_PNG.to_vec()))
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0));

        let brand = row![
            logo_image,
            text("CORTEN")
                .size(15.0)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.primary.to_iced()),
            text("• Hoş Geldiniz Sunumu")
                .size(13.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        // Interactive Slide Dots: ● ○ ○ ○ ○
        let mut dots_row = row![].spacing(Spacing::SM).align_y(Alignment::Center);
        for s in Slide::ALL {
            let is_current = self.slide == s;
            let dot = button(
                container(row![])
                    .width(Length::Fixed(if is_current { 26.0 } else { 8.0 }))
                    .height(Length::Fixed(8.0))
                    .style(move |_| {
                        let bg = if is_current {
                            palette.primary.to_iced()
                        } else {
                            Color::from_rgba(1.0, 1.0, 1.0, 0.22)
                        };
                        container::Style {
                            background: Some(bg.into()),
                            border: Border {
                                radius: border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            )
            .padding(0)
            .style(|_, _| button::Style::default())
            .on_press(Message::GoToSlide(s));
            dots_row = dots_row.push(dot);
        }

        // Persistent Skip / Desktop Launch Button in Header
        let skip_button = button(
            row![
                text("Masaüstüne Geç")
                    .size(12)
                    .color(palette.text_muted.to_iced()),
                text("→")
                    .size(14)
                    .color(palette.primary.to_iced()),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([4, 12])
        .on_press(Message::CompleteWelcome)
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.surface_hover.to_iced(),
                _ => Color::from_rgba(1.0, 1.0, 1.0, 0.06),
            };
            button::Style {
                background: Some(bg.into()),
                border: Border {
                    color: Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            }
        });

        let presentation_header = row![
            brand,
            horizontal_space(),
            dots_row,
            horizontal_space().width(Length::Fixed(20.0)),
            skip_button,
        ]
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .padding([0.0, Spacing::SM]);

        // Main Slide Canvas
        let slide_content: Element<'_, Message> = match self.slide {
            Slide::Welcome => self.render_slide_welcome(palette, pulse),
            Slide::Aesthetics => self.render_slide_aesthetics(palette, pulse),
            Slide::DockAndNavigation => self.render_slide_dock(palette),
            Slide::Shortcuts => self.render_slide_shortcuts(palette),
            Slide::AuraAi => self.render_slide_aura_ai(palette),
            Slide::Ready => self.render_slide_ready(palette, pulse),
        };

        // Bottom Action Bar
        let prev_btn: Element<'_, Message> = if self.slide != Slide::Welcome {
            button(
                row![
                    text(Icon::CHEVRON_RIGHT)
                        .font(LUCIDE_FONT)
                        .size(14)
                        .color(palette.text_muted.to_iced()),
                    text("Önceki Slayt")
                        .size(13)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM, Spacing::MD])
            .on_press(Message::PrevSlide)
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => palette.surface_hover.to_iced(),
                    _ => Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                };
                button::Style {
                    background: Some(bg.into()),
                    border: Border {
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        width: 1.0,
                        radius: border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                }
            })
            .into()
        } else {
            horizontal_space().width(Length::Fixed(100.0)).into()
        };

        let dont_show_box = button(
            row![
                container(
                    container(row![])
                        .width(Length::Fixed(10.0))
                        .height(Length::Fixed(10.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(if self.dont_show_again {
                                Color::WHITE
                            } else {
                                Color::from_rgba(1.0, 1.0, 1.0, 0.4)
                            })),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                )
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(16.0))
                .padding(3)
                .align_x(if self.dont_show_again {
                    iced::alignment::Horizontal::Right
                } else {
                    iced::alignment::Horizontal::Left
                })
                .align_y(iced::alignment::Vertical::Center)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(if self.dont_show_again {
                        palette.primary.to_iced()
                    } else {
                        Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                    })),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                text("Bu sunumu bir daha gösterme")
                    .size(12)
                    .color(if self.dont_show_again {
                        palette.text_primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleDontShowAgain(!self.dont_show_again));

        // On the final slide (Slide::Ready), keep the bottom-right slot as a passive indicator
        // so rapid clicks on "Sonraki Slayt" never accidentally close the wizard before the user
        // reads the final screen and clicks the central "Masaüstünü Başlat" button.
        let next_btn: Element<'_, Message> = if self.slide == Slide::Ready {
            container(
                row![
                    text(Icon::CHECK)
                        .font(LUCIDE_FONT)
                        .size(14)
                        .color(palette.primary.to_iced()),
                    text("Son Adım")
                        .size(13)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM + 2.0, Spacing::LG])
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.04).into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
        } else {
            button(
                row![
                    text("Sonraki Slayt")
                        .size(14)
                        .font(iced::Font {
                            weight: iced::font::Weight::Semibold,
                            ..Default::default()
                        })
                        .color(palette.on_primary().to_iced()),
                    text("→").size(15).color(palette.on_primary().to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM + 2.0, Spacing::LG])
            .on_press(Message::NextSlide)
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => palette.primary_hover.to_iced(),
                    _ => palette.primary.to_iced(),
                };
                button::Style {
                    background: Some(bg.into()),
                    border: Border {
                        radius: border::Radius::from(Radius::MD),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .into()
        };

        let action_bar = row![prev_btn, horizontal_space(), dont_show_box, horizontal_space(), next_btn]
            .width(Length::Fill)
            .align_y(Alignment::Center);

        // Keynote Presentation Theater Stage
        let is_dark_mode = self.theme_mode == ThemeMode::Dark;
        let stage_card = container(
            column![
                presentation_header,
                vertical_space().height(Spacing::MD),
                container(slide_content)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
                vertical_space().height(Spacing::MD),
                action_bar,
            ]
            .spacing(Spacing::XS)
            .width(Length::Fixed(960.0))
            .height(Length::Fixed(600.0)),
        )
        .padding(Spacing::LG)
        .style(move |_| {
            let border_glow = 0.35 + 0.20 * pulse;
            container::Style {
                background: Some(
                    if is_dark_mode {
                        Color::from_rgba(0.08, 0.09, 0.12, 0.96)
                    } else {
                        Color::from_rgba(0.96, 0.97, 0.98, 0.97)
                    }
                    .into(),
                ),
                border: Border {
                    color: Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        border_glow,
                    ),
                    width: 1.5,
                    radius: border::Radius::from(Radius::LG),
                },
                ..Default::default()
            }
        });

        // Full Screen Dark Backdrop
        container(stage_card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(0.03, 0.04, 0.05, 0.98).into()),
                ..Default::default()
            })
            .into()
    }
}

impl CortenWelcomeApp {
    // --- Slide 1: Welcome ---
    fn render_slide_welcome(&self, palette: Palette, pulse: f32) -> Element<'_, Message> {
        let emblem_glow_alpha = 0.10 + 0.15 * pulse;
        let emblem_border_alpha = 0.45 + 0.45 * pulse;

        let emblem = container(
            image(image::Handle::from_bytes(LOGO_PNG.to_vec()))
                .width(Length::Fixed(72.0))
                .height(Length::Fixed(72.0)),
        )
        .padding(Spacing::MD)
        .style(move |_| container::Style {
            background: Some(
                Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    emblem_glow_alpha,
                )
                .into(),
            ),
            border: Border {
                color: Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    emblem_border_alpha,
                ),
                width: 1.5 + 1.0 * pulse,
                radius: border::Radius::from(24.0),
            },
            ..Default::default()
        });

        let feature_pill = |icon_char: char, title: &'static str, desc: &'static str| {
            container(
                column![
                    row![
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(18)
                            .color(palette.primary.to_iced()),
                        text(title)
                            .size(Typography::TITLE_SM)
                            .font(iced::Font {
                                weight: iced::font::Weight::Semibold,
                                ..Default::default()
                            })
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text(desc)
                        .size(11)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(4),
            )
            .width(Length::FillPortion(1))
            .padding(Spacing::MD)
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.04).into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        };

        let hw_badge = |label: &'static str, val: &str| {
            row![
                text(label)
                    .size(11)
                    .color(palette.text_muted.to_iced()),
                text(val.to_string())
                    .size(11)
                    .font(iced::Font {
                        weight: iced::font::Weight::Semibold,
                        ..Default::default()
                    })
                    .color(palette.primary.to_iced()),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
        };

        let hardware_bar = container(
            row![
                text(Icon::MONITOR)
                    .font(LUCIDE_FONT)
                    .size(13)
                    .color(palette.primary.to_iced()),
                hw_badge("İşlemci:", &self.system_info.cpu),
                text("•").size(10).color(palette.text_muted.to_iced()),
                hw_badge("Bellek:", &self.system_info.ram),
                text("•").size(10).color(palette.text_muted.to_iced()),
                hw_badge("Grafik:", &self.system_info.gpu),
                text("•").size(10).color(palette.text_muted.to_iced()),
                hw_badge("Çekirdek:", &self.system_info.kernel),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding([Spacing::XS, Spacing::MD])
        .style(move |_| container::Style {
            background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.03).into()),
            border: Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: border::Radius::from(Radius::FULL),
            },
            ..Default::default()
        });

        column![
            emblem,
            vertical_space().height(Spacing::SM),
            text("Corten Masaüstüne Hoş Geldiniz")
                .size(28)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Saf Rust ve Wayland mimarisiyle sıfırdan inşa edilmiş, minimalist endüstriyel zarafet ve ödün vermeyen performans.")
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            vertical_space().height(Spacing::MD),
            row![
                feature_pill(Icon::CPU, "Saf Rust & DRM/KMS", "Doğrudan GPU donanımı üzerinde ultra düşük gecikme ve sıfır hantallık."),
                feature_pill(Icon::SUN, "Corten Patina", "Dövme çelik, sıcak bakır ve derin füme akrilik cam katmanları."),
                feature_pill(Icon::SHIELD, "Yerel Güvenlik", "Sıfır telemetri ve arka planda çalışan yapay zeka koruma kalkanı."),
            ]
            .spacing(Spacing::MD)
            .width(Length::Fill),
            vertical_space().height(Spacing::SM),
            hardware_bar,
        ]
        .spacing(Spacing::XS)
        .align_x(Alignment::Center)
        .into()
    }

    // --- Slide 2: Aesthetics & Live Configuration ---
    fn render_slide_aesthetics(&self, palette: Palette, pulse: f32) -> Element<'_, Message> {
        // 4 Patina Variants: Rust, Verdigris, Titanium, Molten
        let mut patina_cards_row = row![].spacing(Spacing::SM).width(Length::Fill);

        for p in PatinaVariant::ALL {
            let is_selected = self.active_patina == p;
            let (prim_c, _) = p.colors();

            let title_text = match p {
                PatinaVariant::Rust => "Corten Rust",
                PatinaVariant::Verdigris => "Patina Verdigris",
                PatinaVariant::Titanium => "Forged Titanium",
                PatinaVariant::Molten => "Molten Gold",
            };

            let subtitle_text = match p {
                PatinaVariant::Rust => "Pas & Akkor Kehribar",
                PatinaVariant::Verdigris => "Zümrüt & Oksit Yeşil",
                PatinaVariant::Titanium => "Titanyum & Gümüş Çelik",
                PatinaVariant::Molten => "Erimiş & Sıcak Altın",
            };

            let card_border_color = if is_selected {
                Color::from_rgba(prim_c.r, prim_c.g, prim_c.b, 0.8 + 0.2 * pulse)
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.08)
            };

            let card_bg = if is_selected {
                Color::from_rgba(prim_c.r, prim_c.g, prim_c.b, 0.16)
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.03)
            };

            let status_badge = if is_selected {
                text("✓ Aktif")
                    .size(11)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..Default::default()
                    })
                    .color(prim_c.to_iced())
            } else {
                text("Seç")
                    .size(11)
                    .color(palette.text_muted.to_iced())
            };

            let swatch = container(row![])
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0))
                .style(move |_| container::Style {
                    background: Some(prim_c.to_iced().into()),
                    border: Border {
                        radius: border::Radius::from(9.0),
                        color: Color::WHITE,
                        width: if is_selected { 1.5 } else { 0.0 },
                    },
                    ..Default::default()
                });

            let card = button(
                container(
                    column![
                        row![swatch, horizontal_space(), status_badge].align_y(Alignment::Center),
                        vertical_space().height(Spacing::XS),
                        text(title_text)
                            .size(13)
                            .font(iced::Font {
                                weight: iced::font::Weight::Bold,
                                ..Default::default()
                            })
                            .color(palette.text_primary.to_iced()),
                        text(subtitle_text)
                            .size(11)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(2),
                )
                .padding(Spacing::SM + 2.0)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(card_bg.into()),
                    border: Border {
                        color: card_border_color,
                        width: if is_selected { 1.5 } else { 1.0 },
                        radius: border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                }),
            )
            .padding(0)
            .width(Length::FillPortion(1))
            .style(|_, _| button::Style::default())
            .on_press(Message::SelectPatina(p));

            patina_cards_row = patina_cards_row.push(card);
        }

        // Dark vs Light Mode Interactive Buttons
        let mode_btn = |mode: ThemeMode, label: &'static str, icon_c: char| {
            let is_sel = self.theme_mode == mode;
            button(
                row![
                    text(icon_c)
                        .font(LUCIDE_FONT)
                        .size(16)
                        .color(if is_sel { palette.primary.to_iced() } else { palette.text_muted.to_iced() }),
                    text(label)
                        .size(13)
                        .font(iced::Font {
                            weight: if is_sel { iced::font::Weight::Bold } else { iced::font::Weight::Normal },
                            ..Default::default()
                        })
                        .color(if is_sel { palette.primary.to_iced() } else { palette.text_primary.to_iced() }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM, Spacing::MD])
            .on_press(Message::SelectThemeMode(mode))
            .style(move |_, status| {
                let bg = if is_sel {
                    Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15)
                } else {
                    match status {
                        button::Status::Hovered => palette.surface_hover.to_iced(),
                        _ => Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    }
                };
                button::Style {
                    background: Some(bg.into()),
                    border: Border {
                        color: if is_sel { palette.primary.to_iced() } else { Color::from_rgba(1.0, 1.0, 1.0, 0.1) },
                        width: 1.0,
                        radius: border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                }
            })
        };

        let mode_row = row![
            mode_btn(ThemeMode::Dark, "Koyu Mod (Derin Akrilik)", Icon::MOON),
            mode_btn(ThemeMode::Light, "Açık Mod (Dövme Çelik)", Icon::SUN),
            horizontal_space(),
            button(
                row![
                    container(
                        container(row![])
                            .width(Length::Fixed(10.0))
                            .height(Length::Fixed(10.0))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if self.night_light {
                                    Color::WHITE
                                } else {
                                    Color::from_rgba(1.0, 1.0, 1.0, 0.4)
                                })),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                    )
                    .width(Length::Fixed(28.0))
                    .height(Length::Fixed(16.0))
                    .padding(3)
                    .align_x(if self.night_light {
                        iced::alignment::Horizontal::Right
                    } else {
                        iced::alignment::Horizontal::Left
                    })
                    .align_y(iced::alignment::Vertical::Center)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(if self.night_light {
                            palette.primary.to_iced()
                        } else {
                            Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                        })),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    text("Gece Işığı (Sıcak Ton)")
                        .size(12)
                        .color(if self.night_light {
                            palette.text_primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(0)
            .style(|_, _| button::Style::default())
            .on_press(Message::ToggleNightLight(!self.night_light)),
        ]
        .spacing(Spacing::MD)
        .align_y(Alignment::Center)
        .width(Length::Fill);

        // Feature preview notes
        let preview_note = container(
            row![
                text(Icon::CHECK)
                    .font(LUCIDE_FONT)
                    .size(16)
                    .color(palette.primary.to_iced()),
                text("Seçtiğiniz renk paleti ve tema modu anında uygulanır ve tüm masaüstü bileşenlerine kaydedilir.")
                    .size(12)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding(Spacing::SM)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.08).into()),
            border: Border {
                color: Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.25),
                width: 1.0,
                radius: border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        column![
            text("Kişiselleştirme ve Tasarım Seçimi")
                .size(26)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Masaüstünüzün görsel kimliğini belirleyin; pencere kenarları, dock vurguları ve menüler anında güncellenir:")
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            vertical_space().height(Spacing::SM),
            text("1. Patina Metalik Vurgu Rengi")
                .size(13)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..Default::default()
                })
                .color(palette.primary.to_iced()),
            patina_cards_row,
            vertical_space().height(Spacing::SM),
            text("2. Arayüz Modu & Ekran Konforu")
                .size(13)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..Default::default()
                })
                .color(palette.primary.to_iced()),
            mode_row,
            vertical_space().height(Spacing::SM),
            preview_note,
        ]
        .spacing(Spacing::XS)
        .align_x(Alignment::Start)
        .width(Length::Fill)
        .into()
    }

    // --- Slide 3: Dock & Navigation ---
    fn render_slide_dock(&self, palette: Palette) -> Element<'_, Message> {
        let card = |icon_char: char, title: &'static str, desc: &'static str| {
            container(
                row![
                    container(
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(22)
                            .color(palette.primary.to_iced())
                    )
                    .padding(Spacing::MD)
                    .style(move |_| container::Style {
                        background: Some(Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.12).into()),
                        border: Border {
                            color: Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                            width: 1.0,
                            radius: border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    }),
                    column![
                        text(title)
                            .size(Typography::TITLE_SM)
                            .font(iced::Font {
                                weight: iced::font::Weight::Semibold,
                                ..Default::default()
                            })
                            .color(palette.text_primary.to_iced()),
                        text(desc)
                            .size(12)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(3),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Spacing::MD)
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.04).into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        };

        column![
            text("Merkezi Kapsül Dock & Ayrı Başlatıcı")
                .size(26)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Masaüstünüzü yönetmenin en zarif ve doğrudan yolu:")
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            vertical_space().height(Spacing::SM),
            card(
                Icon::APP_WINDOW,
                "Ayrı Corten Başlatıcı Butonu (Orb)",
                "Dock'un solundaki bağımsız Corten orb'una tıklayarak veya klavyenizdeki Super tuşuna basarak ekranın merkezinde açılan başlatıcıya erişin."
            ),
            card(
                Icon::TERMINAL,
                "Dinamik Boyutlanan Kapsül Dock",
                "Sabitlenmiş uygulamalarınız (Terminal, Dosya Gezgini, Ayarlar, Kod Düzenleyici, Tarayıcı) ve çalışan pencerelerinizle senkronize şekilde boyutlanır."
            ),
            card(
                Icon::CLOCK,
                "Modern Masaüstü Desklet",
                "Sağ üst köşedeki minimalist cam widget, saat, takvim ve CPU/RAM kullanımınızı kesintisiz izlemenizi sağlar."
            ),
        ]
        .spacing(Spacing::SM)
        .align_x(Alignment::Start)
        .width(Length::Fill)
        .into()
    }

    // --- Slide 4: Shortcuts ---
    fn render_slide_shortcuts(&self, palette: Palette) -> Element<'_, Message> {
        let shortcut_tile = |key: &'static str, desc: &'static str| {
            container(
                row![
                    container(
                        text(key)
                            .size(12)
                            .font(iced::Font {
                                weight: iced::font::Weight::Bold,
                                ..Default::default()
                            })
                            .color(palette.primary.to_iced())
                    )
                    .padding([4, 10])
                    .style(move |_| container::Style {
                        background: Some(Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.14).into()),
                        border: Border {
                            color: palette.primary.to_iced(),
                            width: 1.0,
                            radius: border::Radius::from(Radius::SM),
                        },
                        ..Default::default()
                    }),
                    text(desc)
                        .size(13)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
            )
            .width(Length::FillPortion(1))
            .padding(Spacing::MD)
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.04).into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        };

        column![
            text("Işık Hızında Klavye İş Akışı")
                .size(26)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Elinizi klavyeden kaldırmadan tüm sistemi yönetmenizi sağlayan temel kısayollar:")
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            vertical_space().height(Spacing::SM),
            row![
                shortcut_tile("Super + Space", "Spotlight Başlatıcı & Arama"),
                shortcut_tile("Super + Return", "Hızlı GPU Terminal (Alacritty)"),
            ]
            .spacing(Spacing::MD),
            row![
                shortcut_tile("Super + E", "Corten Dosya Gezgini"),
                shortcut_tile("Alt + Tab", "Akrilik Pencere Değiştirici"),
            ]
            .spacing(Spacing::MD),
            row![
                shortcut_tile("Super + N", "Hızlı Not Defteri"),
                shortcut_tile("Super + Q", "Aktif Pencereyi Kapat"),
            ]
            .spacing(Spacing::MD),
        ]
        .spacing(Spacing::SM)
        .align_x(Alignment::Start)
        .width(Length::Fill)
        .into()
    }

    // --- Slide 5: AURA-CORE Local AI & Security ---
    fn render_slide_aura_ai(&self, palette: Palette) -> Element<'_, Message> {
        let card = |icon_char: char, title: &'static str, desc: &'static str| {
            container(
                row![
                    container(
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(22)
                            .color(palette.primary.to_iced())
                    )
                    .padding(Spacing::MD)
                    .style(move |_| container::Style {
                        background: Some(
                            Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.12,
                            )
                            .into(),
                        ),
                        border: Border {
                            color: Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.35,
                            ),
                            width: 1.0,
                            radius: border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    }),
                    column![
                        text(title)
                            .size(Typography::TITLE_SM)
                            .font(iced::Font {
                                weight: iced::font::Weight::Semibold,
                                ..Default::default()
                            })
                            .color(palette.text_primary.to_iced()),
                        text(desc).size(12).color(palette.text_muted.to_iced()),
                    ]
                    .spacing(3),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Spacing::MD)
            .style(move |_| container::Style {
                background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.04).into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        };

        column![
            text("AURA-CORE Yerel Yapay Zekâ & Güvenlik")
                .size(26)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Harici bulut veya Python bağımlılığı olmadan tamamen cihazınızda çalışan Saf Rust yapay zekâ motoru:")
                .size(Typography::BODY)
                .color(palette.text_muted.to_iced()),
            vertical_space().height(Spacing::SM),
            card(
                Icon::CPU,
                "Saf Rust GGUF & HNSW Vektör Motoru (Super + Alt + Space)",
                "corten-llm ve corten-vector ile yerel GGUF modelleri, WAL kalıcılıklı uzun süreli hafıza ve kod tabanı RAG araması."
            ),
            card(
                Icon::TERMINAL,
                "20 Yerleşik Masaüstü & DevOps Aracı (ReAct Ajanı)",
                "Çok adımlı Thought -> Action -> Observation döngüsüyle pencere, çalışma alanı, ses, parlaklık, Git ve süreç yönetimi."
            ),
            card(
                Icon::SHIELD,
                "HITL (İnsan Onaylı) Güvenlik Kapısı & Denetim İzi",
                "Yıkıcı veya kritik işlemler asla otomatik çalıştırılmaz; onayınıza sunulur ve hitl_audit.log dosyasına kaydedilir."
            ),
        ]
        .spacing(Spacing::SM)
        .align_x(Alignment::Start)
        .width(Length::Fill)
        .into()
    }

    // --- Slide 6: Ready ---
    fn render_slide_ready(&self, palette: Palette, pulse: f32) -> Element<'_, Message> {
        let emblem_glow_alpha = 0.12 + 0.18 * pulse;
        let emblem_border_alpha = 0.50 + 0.50 * pulse;

        let emblem = container(
            image(image::Handle::from_bytes(LOGO_PNG.to_vec()))
                .width(Length::Fixed(84.0))
                .height(Length::Fixed(84.0)),
        )
        .padding(Spacing::LG)
        .style(move |_| container::Style {
            background: Some(
                Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    emblem_glow_alpha,
                )
                .into(),
            ),
            border: Border {
                color: Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    emblem_border_alpha,
                ),
                width: 2.0 + 1.0 * pulse,
                radius: border::Radius::from(32.0),
            },
            ..Default::default()
        });

        column![
            emblem,
            vertical_space().height(Spacing::SM),
            text("Artık Hazırsınız!")
                .size(30)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            text("Corten Masaüstü ortamınız tüm bileşenleriyle kullanıma hazır.")
                .size(15)
                .color(palette.text_muted.to_iced()),
            text("Aşağıdaki butona tıklayarak doğrudan masaüstünüze geçiş yapabilirsiniz.")
                .size(13)
                .color(palette.primary.to_iced()),
            vertical_space().height(Spacing::MD),
            button(
                row![
                    text("Masaüstünü Başlat")
                        .size(16)
                        .font(iced::Font {
                            weight: iced::font::Weight::Bold,
                            ..Default::default()
                        })
                        .color(palette.on_primary().to_iced()),
                    text("→").size(18).color(palette.on_primary().to_iced()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::MD, Spacing::XL])
            .on_press(Message::CompleteWelcome)
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => palette.primary_hover.to_iced(),
                    _ => palette.primary.to_iced(),
                };
                button::Style {
                    background: Some(bg.into()),
                    border: Border {
                        radius: border::Radius::from(Radius::LG),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }),
        ]
        .spacing(Spacing::XS)
        .align_x(Alignment::Center)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slide_progression_and_reversal() {
        let mut slide = Slide::Welcome;
        assert_eq!(slide.prev(), None);
        assert_eq!(slide.next(), Some(Slide::Aesthetics));

        slide = slide.next().unwrap();
        assert_eq!(slide, Slide::Aesthetics);
        assert_eq!(slide.next(), Some(Slide::DockAndNavigation));

        slide = slide.next().unwrap();
        assert_eq!(slide, Slide::DockAndNavigation);
        assert_eq!(slide.next(), Some(Slide::Shortcuts));

        slide = slide.next().unwrap();
        assert_eq!(slide, Slide::Shortcuts);
        assert_eq!(slide.next(), Some(Slide::AuraAi));

        slide = slide.next().unwrap();
        assert_eq!(slide, Slide::AuraAi);
        assert_eq!(slide.next(), Some(Slide::Ready));

        slide = slide.next().unwrap();
        assert_eq!(slide, Slide::Ready);
        assert_eq!(slide.next(), None);
        assert_eq!(slide.prev(), Some(Slide::AuraAi));
    }

    #[test]
    fn test_default_app_state() {
        let app = CortenWelcomeApp::default();
        assert_eq!(app.slide, Slide::Welcome);
        assert!(app.dont_show_again);
        assert!(!app.system_info.cpu.is_empty());
        assert!(!app.system_info.kernel.is_empty());
    }

    #[test]
    fn test_toggle_night_light_message() {
        let mut app = CortenWelcomeApp::default();
        let initial = app.night_light;
        let _ = app.update(Message::ToggleNightLight(!initial));
        assert_eq!(app.night_light, !initial);
        let _ = app.update(Message::ToggleNightLight(initial));
        assert_eq!(app.night_light, initial);
    }

    #[test]
    fn test_slide_navigation_messages() {
        let mut app = CortenWelcomeApp::default();
        assert_eq!(app.slide, Slide::Welcome);

        let _ = app.update(Message::NextSlide);
        assert_eq!(app.slide, Slide::Aesthetics);

        let _ = app.update(Message::NextSlide);
        assert_eq!(app.slide, Slide::DockAndNavigation);

        let _ = app.update(Message::PrevSlide);
        assert_eq!(app.slide, Slide::Aesthetics);

        let _ = app.update(Message::GoToSlide(Slide::Ready));
        assert_eq!(app.slide, Slide::Ready);
    }
}
