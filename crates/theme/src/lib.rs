//! # Corten Theme
//! Design tokens, palette definitions, and iced widget style helpers for Corten Desktop.

pub mod animation;
pub mod bar;
pub mod color;
pub mod icons;
pub mod radius;
pub mod spacing;
pub mod typography;

#[cfg(feature = "iced")]
pub mod iced_styles;

#[cfg(feature = "iced")]
pub use iced_styles::SurfaceLevel;

pub use animation::{Motion, Transition};
pub use bar::BarTokens;
pub use color::{Color, ColorFilterMode, Palette, PatinaVariant, ThemeMode};
pub use icons::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BOLD, NOTO_SANS_BYTES,
    NOTO_SANS_MEDIUM, NOTO_SANS_SEMIBOLD,
};
pub use radius::Radius;
pub use spacing::{Density, Spacing};
pub use typography::Typography;
