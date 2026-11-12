#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// 4-point spacing scale for layouts and widget paddings
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing;

impl Spacing {
    pub const XXS: f32 = 4.0;
    pub const XS: f32 = 8.0;
    pub const SM: f32 = 12.0;
    pub const MD: f32 = 16.0;
    pub const LG: f32 = 24.0;
    pub const XL: f32 = 32.0;
    pub const XXL: f32 = 48.0;
}

/// UI Information Density scale
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    pub const ALL: [Self; 2] = [Self::Comfortable, Self::Compact];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Comfortable => "Ferah (Comfortable)",
            Self::Compact => "Kompakt (Compact)",
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Comfortable => "comfortable",
            Self::Compact => "compact",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.to_lowercase().as_str() {
            "compact" | "dense" | "tight" => Self::Compact,
            _ => Self::Comfortable,
        }
    }

    pub fn row_height(&self) -> f32 {
        match self {
            Self::Comfortable => 48.0,
            Self::Compact => 36.0,
        }
    }

    pub fn item_padding(&self) -> f32 {
        match self {
            Self::Comfortable => Spacing::SM,
            Self::Compact => Spacing::XS,
        }
    }

    pub fn item_spacing(&self) -> f32 {
        match self {
            Self::Comfortable => Spacing::SM,
            Self::Compact => Spacing::XXS + 2.0,
        }
    }

    pub fn icon_size(&self) -> f32 {
        match self {
            Self::Comfortable => 22.0,
            Self::Compact => 17.0,
        }
    }

    pub fn font_size_body(&self) -> f32 {
        match self {
            Self::Comfortable => 14.0,
            Self::Compact => 12.5,
        }
    }
}
