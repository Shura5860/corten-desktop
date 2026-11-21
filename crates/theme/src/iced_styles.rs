use iced::widget::{button, container};
use iced::{border, Background, Border, Shadow};

use crate::color::Palette;
use crate::radius::Radius;

/// Container style for cards and panels with explicit border separation
pub fn card_style(palette: Palette) -> container::Style {
    surface_style(palette, false)
}

/// Shared surface style for cards, panels, and popovers.
pub fn surface_style(palette: Palette, elevated: bool) -> container::Style {
    container::Style {
        text_color: Some(palette.text_primary.to_iced()),
        background: Some(Background::Color(palette.surface.to_iced())),
        border: Border {
            color: palette.border.to_iced(),
            width: 1.0,
            radius: border::Radius::from(Radius::MD),
        },
        shadow: if elevated {
            Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.28),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 14.0,
            }
        } else {
            Shadow::default()
        },
    }
}

/// Elevated surface style for transient overlays and popovers.
pub fn popover_style(palette: Palette) -> container::Style {
    surface_style(palette, true)
}

/// Standardized elevation & glass surface hierarchy across all Corten applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceLevel {
    /// Flat base container (`background`)
    Base,
    /// Standard card or sidebar (`surface`, 1px border)
    Raised,
    /// Floating modal / popover (`surface` + elevation shadow)
    Overlay,
    /// Frosted acrylic glass (`surface` translucent + patina highlight border + ambient shadow)
    Glass,
}

/// Returns a consistent `container::Style` for the requested `SurfaceLevel` in both Dark & Light modes.
pub fn surface_level_style(palette: Palette, level: SurfaceLevel, radius: f32) -> container::Style {
    match level {
        SurfaceLevel::Base => container::Style {
            text_color: Some(palette.text_primary.to_iced()),
            background: Some(Background::Color(palette.background.to_iced())),
            border: Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: border::Radius::from(radius),
            },
            shadow: Shadow::default(),
        },
        SurfaceLevel::Raised => container::Style {
            text_color: Some(palette.text_primary.to_iced()),
            background: Some(Background::Color(palette.surface.to_iced())),
            border: Border {
                color: palette.border.to_iced(),
                width: 1.0,
                radius: border::Radius::from(radius),
            },
            shadow: Shadow::default(),
        },
        SurfaceLevel::Overlay => {
            let shadow_alpha = if palette.is_light() { 0.14 } else { 0.30 };
            container::Style {
                text_color: Some(palette.text_primary.to_iced()),
                background: Some(Background::Color(palette.surface.to_iced())),
                border: Border {
                    color: palette.border.to_iced(),
                    width: 1.0,
                    radius: border::Radius::from(radius),
                },
                shadow: Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, shadow_alpha),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 14.0,
                },
            }
        }
        SurfaceLevel::Glass => acrylic_glass_style(palette, radius),
    }
}

/// High-end frosted glass acrylic style with patina border highlight and ambient shadow
pub fn acrylic_glass_style(palette: Palette, radius: f32) -> container::Style {
    let mut bg = palette.surface.to_iced();
    bg.a = if palette.is_light() { 0.96 } else { 0.93 };
    let border_alpha = if palette.is_light() { 0.40 } else { 0.32 };
    let shadow_alpha = if palette.is_light() { 0.16 } else { 0.35 };
    container::Style {
        text_color: Some(palette.text_primary.to_iced()),
        background: Some(Background::Color(bg)),
        border: Border {
            color: iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                border_alpha,
            ),
            width: 1.0,
            radius: border::Radius::from(radius),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, shadow_alpha),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 16.0,
        },
    }
}

/// Tactile Bento tile button style for Control Center quick toggles and launcher favorite tiles
pub fn bento_tile_button_style(
    palette: Palette,
    is_active: bool,
    status: button::Status,
) -> button::Style {
    let (bg, border_color, text_color) = if is_active {
        let bg = match status {
            button::Status::Hovered => palette.primary_hover.to_iced(),
            _ => iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                0.22,
            ),
        };
        let fg = if matches!(status, button::Status::Hovered) {
            palette.on_primary().to_iced()
        } else {
            palette.text_primary.to_iced()
        };
        (bg, palette.primary.to_iced(), fg)
    } else {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => palette.surface_hover.to_iced(),
            _ => palette.surface.to_iced(),
        };
        (bg, palette.border.to_iced(), palette.text_primary.to_iced())
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: border::Radius::from(Radius::MD),
        },
        shadow: Shadow::default(),
    }
}

/// Active halo glow border style for focused windows and selected items
pub fn halo_glow_border(palette: Palette, radius: f32) -> Border {
    Border {
        color: palette.primary.to_iced(),
        width: 1.5,
        radius: border::Radius::from(radius),
    }
}

/// Input surface style for search fields and text-entry controls.
pub fn input_style(palette: Palette) -> container::Style {
    container::Style {
        text_color: Some(palette.text_primary.to_iced()),
        background: Some(Background::Color(palette.surface_hover.to_iced())),
        border: Border {
            color: palette.border.to_iced(),
            width: 1.0,
            radius: border::Radius::from(Radius::SM),
        },
        shadow: Shadow::default(),
    }
}

/// Primary button style with Corten Rust accent and clean rounded corners
pub fn primary_button_style(palette: Palette, status: button::Status) -> button::Style {
    let on = palette.on_primary().to_iced();
    let (bg, text_color) = match status {
        button::Status::Active => (palette.primary.to_iced(), on),
        button::Status::Hovered => (palette.primary_hover.to_iced(), on),
        button::Status::Pressed => (palette.primary.to_iced(), on),
        button::Status::Disabled => (
            palette.surface_hover.to_iced(),
            palette.text_disabled.to_iced(),
        ),
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: if status == button::Status::Disabled {
                palette.border.to_iced()
            } else {
                iced::Color::TRANSPARENT
            },
            width: if status == button::Status::Disabled {
                1.0
            } else {
                0.0
            },
            radius: border::Radius::from(Radius::MD),
        },
        shadow: Shadow::default(),
    }
}

/// Secondary button style with explicit border
pub fn secondary_button_style(palette: Palette, status: button::Status) -> button::Style {
    let (bg, text_color) = match status {
        button::Status::Active => (palette.surface.to_iced(), palette.text_primary.to_iced()),
        button::Status::Hovered => (
            palette.surface_hover.to_iced(),
            palette.text_primary.to_iced(),
        ),
        button::Status::Pressed => (palette.surface.to_iced(), palette.text_primary.to_iced()),
        button::Status::Disabled => (
            palette.background.to_iced(),
            palette.text_disabled.to_iced(),
        ),
    };

    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: palette.border.to_iced(),
            width: 1.0,
            radius: border::Radius::from(Radius::MD),
        },
        shadow: Shadow::default(),
    }
}

/// Tag/Badge container style for status indicators
pub fn badge_style(palette: Palette, is_accent: bool) -> container::Style {
    container::Style {
        text_color: Some(if is_accent {
            palette.primary.to_iced()
        } else {
            palette.text_muted.to_iced()
        }),
        background: Some(Background::Color(if is_accent {
            iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                0.15,
            )
        } else {
            palette.surface_hover.to_iced()
        })),
        border: Border {
            color: if is_accent {
                iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.35,
                )
            } else {
                palette.border.to_iced()
            },
            width: 1.0,
            radius: border::Radius::from(Radius::SM),
        },
        shadow: Shadow::default(),
    }
}

/// Keyboard key badge style (e.g. ⌘ / ↵ / Tab)
pub fn kbd_style(palette: Palette) -> container::Style {
    container::Style {
        text_color: Some(palette.text_primary.to_iced()),
        background: Some(Background::Color(palette.surface_hover.to_iced())),
        border: Border {
            color: palette.border.to_iced(),
            width: 1.0,
            radius: border::Radius::from(Radius::SM),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.20),
            offset: iced::Vector::new(0.0, 1.0),
            blur_radius: 1.0,
        },
    }
}

/// Fluid capsule slider style with filled patina rail and glowing thumb (macOS/iOS inspired)
pub fn capsule_slider_style(palette: Palette) -> impl Fn(&iced::Theme, iced::widget::slider::Status) -> iced::widget::slider::Style {
    move |_theme, status| {
        let (handle_color, rail_active) = match status {
            iced::widget::slider::Status::Hovered => (
                palette.primary_hover.to_iced(),
                palette.primary_hover.to_iced(),
            ),
            iced::widget::slider::Status::Dragged => (
                iced::Color::WHITE,
                palette.primary.to_iced(),
            ),
            _ => (
                palette.primary.to_iced(),
                palette.primary.to_iced(),
            ),
        };

        iced::widget::slider::Style {
            rail: iced::widget::slider::Rail {
                backgrounds: (
                    Background::Color(rail_active),
                    Background::Color(palette.surface_hover.to_iced()),
                ),
                width: 10.0,
                border: Border {
                    radius: border::Radius::from(5.0),
                    width: 0.0,
                    color: iced::Color::TRANSPARENT,
                },
            },
            handle: iced::widget::slider::Handle {
                shape: iced::widget::slider::HandleShape::Circle { radius: 8.0 },
                background: Background::Color(handle_color),
                border_width: 2.0,
                border_color: palette.surface.to_iced(),
            },
        }
    }
}

/// Shared interactive pill switch widget usable across all Corten apps
pub fn pill_switch<'a, Message: Clone + 'a>(
    is_active: bool,
    on_toggle: Message,
    palette: Palette,
) -> iced::Element<'a, Message> {
    use iced::widget::{horizontal_space, row};
    use iced::{Alignment, Length};

    let (track_color, border_color) = if is_active {
        (palette.primary.to_iced(), palette.primary.to_iced())
    } else {
        (palette.surface_hover.to_iced(), palette.border.to_iced())
    };

    let thumb_color = if is_active {
        palette.on_primary().to_iced()
    } else {
        palette.text_muted.to_iced()
    };

    let thumb = container(row![])
        .width(Length::Fixed(18.0))
        .height(Length::Fixed(18.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(thumb_color)),
            border: Border {
                radius: border::Radius::from(9.0),
                ..Default::default()
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
            .width(Length::Fixed(44.0))
            .height(Length::Fixed(24.0))
            .padding(2.0)
            .style(move |_| container::Style {
                background: Some(Background::Color(track_color)),
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

/// Unified empty-state placeholder card with centered Lucide icon, title and muted hint.
pub fn empty_state_card<'a, Message: 'a>(
    icon: char,
    title: &'a str,
    subtitle: &'a str,
    palette: Palette,
) -> iced::Element<'a, Message> {
    use crate::icons::LUCIDE_FONT;
    use iced::widget::{column, text};
    use iced::{Alignment, Length};

    container(
        column![
            text(icon)
                .font(LUCIDE_FONT)
                .size(28.0)
                .color(palette.text_muted.to_iced()),
            text(title)
                .size(14.0)
                .color(palette.text_primary.to_iced()),
            text(subtitle)
                .size(12.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(6)
        .align_x(Alignment::Center),
    )
    .padding(24)
    .width(Length::Fill)
    .center_x(Length::Fill)
    .style(move |_| surface_level_style(palette, SurfaceLevel::Raised, Radius::MD))
    .into()
}

/// Unified compact toast/snackbar banner for action feedback across Corten applications.
pub fn toast_banner<'a, Message: 'a>(
    icon: char,
    message: &'a str,
    palette: Palette,
) -> iced::Element<'a, Message> {
    use crate::icons::LUCIDE_FONT;
    use iced::widget::{row, text};
    use iced::Alignment;

    container(
        row![
            text(icon)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text(message)
                .size(12.5)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 14])
    .style(move |_| surface_level_style(palette, SurfaceLevel::Glass, Radius::FULL))
    .into()
}
