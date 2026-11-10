use corten_theme::iced_styles::{
    badge_style, card_style, kbd_style, primary_button_style, secondary_button_style,
};
use corten_theme::{
    Icon, Palette, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT, LUCIDE_FONT_BYTES,
};
use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length};

pub fn main() -> iced::Result {
    iced::application(
        "Corten Theme Preview",
        ThemePreview::update,
        ThemePreview::view,
    )
    .window(iced::window::Settings {
        size: iced::Size::new(880.0, 680.0),
        resizable: true,
        ..Default::default()
    })
    .font(LUCIDE_FONT_BYTES)
    .run()
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ToggleTheme,
    ActionClicked(&'static str),
}

#[derive(Default)]
struct ThemePreview {
    mode: ThemeMode,
    status_text: String,
}

impl ThemePreview {
    fn update(&mut self, message: Message) {
        match message {
            Message::ToggleTheme => {
                self.mode = match self.mode {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
                self.status_text = format!("Theme switched to: {:?}", self.mode);
            }
            Message::ActionClicked(btn) => {
                self.status_text = format!("Interacted with: {}", btn);
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(self.mode);

        // Header with action-oriented toggle button
        let toggle_icon = match self.mode {
            ThemeMode::Dark => Icon::SUN,
            ThemeMode::Light => Icon::MOON,
        };
        let toggle_label = match self.mode {
            ThemeMode::Dark => "Switch to Light Mode",
            ThemeMode::Light => "Switch to Dark Mode",
        };

        let header = row![
            column![
                text("Corten Design System").size(Typography::TITLE),
                text("Wayland Desktop Environment Visual Preview")
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::XXS),
            button(
                row![
                    text(toggle_icon).font(LUCIDE_FONT).size(Typography::BODY),
                    text(toggle_label).size(Typography::BODY_SM),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center)
            )
            .style(move |_, status| secondary_button_style(palette, status))
            .on_press(Message::ToggleTheme),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill);

        // Color Swatches with distinct borders and luminance contrast
        let swatches = row![
            color_box("Background", palette.background, palette),
            color_box("Surface", palette.surface, palette),
            color_box("Primary Rust", palette.primary, palette),
            color_box("Success", palette.success, palette),
            color_box("Warning", palette.warning, palette),
            color_box("Danger", palette.danger, palette),
        ]
        .spacing(Spacing::SM);

        // Simulated Panel Bar with real Lucide icons
        let simulated_panel = container(
            row![
                row![
                    text(Icon::TERMINAL)
                        .font(LUCIDE_FONT)
                        .size(Typography::BODY),
                    text("Corten OS").size(Typography::BODY),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                text("1 | 2 | 3 | 4")
                    .size(Typography::BODY_SM)
                    .color(palette.text_primary.to_iced()),
                row![
                    text(Icon::MUSIC)
                        .font(LUCIDE_FONT)
                        .size(Typography::CAPTION),
                    text("Synthwave Odyssey - Live").size(Typography::CAPTION),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                row![
                    text(Icon::VOLUME)
                        .font(LUCIDE_FONT)
                        .size(Typography::CAPTION),
                    text("80%").size(Typography::CAPTION),
                    text(Icon::CLOCK)
                        .font(LUCIDE_FONT)
                        .size(Typography::BODY_SM)
                        .color(palette.primary.to_iced()),
                    text("14:30")
                        .size(Typography::BODY_SM)
                        .color(palette.primary.to_iced()),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            ]
            .spacing(Spacing::LG)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .padding(Spacing::SM)
        .style(move |_| card_style(palette));

        // Interactive Card with high-contrast text and disabled states
        let interactive_card = container(
            column![
                text("Component States & Buttons")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
                row![
                    button(text("Primary Button").size(Typography::BODY))
                        .style(move |_, status| primary_button_style(palette, status))
                        .padding([Spacing::XS, Spacing::MD])
                        .on_press(Message::ActionClicked("Primary Button")),
                    button(text("Secondary Button").size(Typography::BODY))
                        .style(move |_, status| secondary_button_style(palette, status))
                        .padding([Spacing::XS, Spacing::MD])
                        .on_press(Message::ActionClicked("Secondary Button")),
                    button(text("Disabled Button").size(Typography::BODY))
                        .style(move |_, status| primary_button_style(palette, status))
                        .padding([Spacing::XS, Spacing::MD]),
                ]
                .spacing(Spacing::MD),
                text(if self.status_text.is_empty() {
                    "Ready. Click buttons or toggle theme to test interactivity."
                } else {
                    &self.status_text
                })
                .size(Typography::BODY_SM)
                .color(palette.primary.to_iced()),
            ]
            .spacing(Spacing::MD),
        )
        .padding(Spacing::MD)
        .width(Length::Fill)
        .style(move |_| card_style(palette));

        // Keyboard & Badge Tokens showcase
        let tokens_showcase = container(
            column![
                text("Badges, Tags & Keyboard Indicators")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
                row![
                    container(text("STABLE v0.1.0").size(Typography::CAPTION))
                        .padding([3, 8])
                        .style(move |_| badge_style(palette, true)),
                    container(text("WAYLAND NATIVE").size(Typography::CAPTION))
                        .padding([3, 8])
                        .style(move |_| badge_style(palette, false)),
                    container(text("ZERO-COST").size(Typography::CAPTION))
                        .padding([3, 8])
                        .style(move |_| badge_style(palette, false)),
                    row![
                        container(text("⌘ Super").size(Typography::CAPTION))
                            .padding([2, 6])
                            .style(move |_| kbd_style(palette)),
                        text("+")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                        container(text("Space").size(Typography::CAPTION))
                            .padding([2, 6])
                            .style(move |_| kbd_style(palette)),
                        text("Launcher")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
            ]
            .spacing(Spacing::SM),
        )
        .padding(Spacing::MD)
        .width(Length::Fill)
        .style(move |_| card_style(palette));

        let content = column![
            header,
            text("Color Palette (WCAG Compliant)")
                .size(Typography::TITLE_SM)
                .color(palette.text_primary.to_iced()),
            swatches,
            text("Simulated Desktop Panel / Status Bar")
                .size(Typography::TITLE_SM)
                .color(palette.text_primary.to_iced()),
            simulated_panel,
            interactive_card,
            tokens_showcase,
        ]
        .spacing(Spacing::MD)
        .padding(Spacing::LG);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(palette.background.to_iced())),
                text_color: Some(palette.text_primary.to_iced()),
                ..Default::default()
            })
            .into()
    }
}

fn color_box<'a>(
    name: &'static str,
    color: corten_theme::Color,
    palette: Palette,
) -> Element<'a, Message> {
    container(
        column![
            container(column![])
                .width(Length::Fixed(70.0))
                .height(Length::Fixed(44.0))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(color.to_iced())),
                    border: iced::Border {
                        color: palette.border.to_iced(),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    shadow: iced::Shadow::default(),
                    ..Default::default()
                }),
            text(name)
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        ]
        .align_x(Alignment::Center)
        .spacing(Spacing::XXS),
    )
    .into()
}
