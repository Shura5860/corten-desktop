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

/// High-end frosted glass acrylic style with specular top lighting highlight and deep ambient blur shadow
pub fn acrylic_glass_style(palette: Palette, radius: f32) -> container::Style {
    container::Style {
        text_color: Some(palette.text_primary.to_iced()),
        background: Some(Background::Color(iced::Color::from_rgba(0.08, 0.09, 0.12, 0.90))),
        border: Border {
            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.16),
            width: 1.0,
            radius: border::Radius::from(radius),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.52),
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
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
    let (bg, text_color) = match status {
        button::Status::Active => (palette.primary.to_iced(), iced::Color::WHITE),
        button::Status::Hovered => (palette.primary_hover.to_iced(), iced::Color::WHITE),
        button::Status::Pressed => (palette.primary.to_iced(), iced::Color::WHITE),
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
                    Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)),
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
                border_color: iced::Color::from_rgba(0.08, 0.09, 0.12, 0.8),
            },
        }
    }
}
