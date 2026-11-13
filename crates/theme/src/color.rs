#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// RGBA Color representation
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn from_rgb(r: f32, g: f32, b: f32) -> Self {
        Self::from_rgba(r, g, b, 1.0)
    }

    pub const fn from_hex(hex: u32) -> Self {
        let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
        let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
        let b = (hex & 0xFF) as f32 / 255.0;
        Self::from_rgb(r, g, b)
    }

    #[cfg(feature = "iced")]
    pub fn to_iced(self) -> iced::Color {
        iced::Color::from_rgba(self.r, self.g, self.b, self.a)
    }

    /// Calculates relative luminance for WCAG contrast ratio calculations
    pub fn relative_luminance(&self) -> f32 {
        fn adjust(channel: f32) -> f32 {
            if channel <= 0.03928 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * adjust(self.r) + 0.7152 * adjust(self.g) + 0.0722 * adjust(self.b)
    }

    /// Computes WCAG contrast ratio between this color and another color (returns between 1.0 and 21.0)
    pub fn contrast_ratio(&self, other: &Self) -> f32 {
        let l1 = self.relative_luminance();
        let l2 = other.relative_luminance();
        let lighter = l1.max(l2);
        let darker = l1.min(l2);
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Transforms color according to accessibility color vision deficiency filters
    pub fn with_color_filter(self, filter: ColorFilterMode) -> Self {
        match filter {
            ColorFilterMode::None => self,
            ColorFilterMode::Monochrome => {
                let l = self.relative_luminance();
                Self::from_rgba(l, l, l, self.a)
            }
            ColorFilterMode::Protanopia => {
                // Reduced red sensitivity: simulate Brettel / Machado LMS projection
                let r = (0.56667 * self.r + 0.43333 * self.g).clamp(0.0, 1.0);
                let g = (0.55833 * self.r + 0.44167 * self.g).clamp(0.0, 1.0);
                let b = (0.24167 * self.g + 0.75833 * self.b).clamp(0.0, 1.0);
                Self::from_rgba(r, g, b, self.a)
            }
            ColorFilterMode::Deuteranopia => {
                // Reduced green sensitivity
                let r = (0.625 * self.r + 0.375 * self.g).clamp(0.0, 1.0);
                let g = (0.700 * self.r + 0.300 * self.g).clamp(0.0, 1.0);
                let b = (0.300 * self.g + 0.700 * self.b).clamp(0.0, 1.0);
                Self::from_rgba(r, g, b, self.a)
            }
            ColorFilterMode::Tritanopia => {
                // Reduced blue sensitivity
                let r = (0.950 * self.r + 0.050 * self.g).clamp(0.0, 1.0);
                let g = (0.43333 * self.g + 0.56667 * self.b).clamp(0.0, 1.0);
                let b = (0.475 * self.g + 0.525 * self.b).clamp(0.0, 1.0);
                Self::from_rgba(r, g, b, self.a)
            }
        }
    }
}

/// Color filter modes for accessibility and visual impairment assistance
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ColorFilterMode {
    #[default]
    None,
    Protanopia,
    Deuteranopia,
    Tritanopia,
    Monochrome,
}

impl ColorFilterMode {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Protanopia,
        Self::Deuteranopia,
        Self::Tritanopia,
        Self::Monochrome,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "Kapalı (Standart)",
            Self::Protanopia => "Protanopi (Kırmızı Körü)",
            Self::Deuteranopia => "Deuteranopi (Yeşil Körü)",
            Self::Tritanopia => "Tritanopi (Mavi Körü)",
            Self::Monochrome => "Monokrom (Gri Tonlama)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum PatinaVariant {
    #[default]
    Rust,
    Verdigris,
    Titanium,
    Molten,
}

impl PatinaVariant {
    pub const ALL: [Self; 4] = [Self::Rust, Self::Verdigris, Self::Titanium, Self::Molten];

    pub fn name(&self) -> &'static str {
        match self {
            Self::Rust => "Corten Rust (Pas & Bakır)",
            Self::Verdigris => "Patina Verdigris (Zümrüt Bakır)",
            Self::Titanium => "Forged Titanium (Gümüş Çelik)",
            Self::Molten => "Molten Gold (Akkor Altın)",
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Verdigris => "verdigris",
            Self::Titanium => "titanium",
            Self::Molten => "molten",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.to_lowercase().as_str() {
            "verdigris" | "green" => Self::Verdigris,
            "titanium" | "blue" | "steel" => Self::Titanium,
            "molten" | "gold" | "amber" => Self::Molten,
            _ => Self::Rust,
        }
    }

    pub fn colors(&self) -> (Color, Color) {
        match self {
            Self::Rust => (Color::from_hex(0xD96B43), Color::from_hex(0xEA7D56)),
            Self::Verdigris => (Color::from_hex(0x3EA892), Color::from_hex(0x4FC5AB)),
            Self::Titanium => (Color::from_hex(0x8C9AA8), Color::from_hex(0xA2B0BE)),
            Self::Molten => (Color::from_hex(0xF5A938), Color::from_hex(0xFBC068)),
        }
    }

    /// Determines the best matching PatinaVariant from an RGB color sample based on hue and saturation
    pub fn match_from_rgb(r: u8, g: u8, b: u8) -> Self {
        let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let delta = max - min;

        if delta < 0.08 {
            // Low saturation: Titanium (forged industrial steel)
            return Self::Titanium;
        }

        let hue = if (max - rf).abs() < f32::EPSILON {
            60.0 * (((gf - bf) / delta) % 6.0)
        } else if (max - gf).abs() < f32::EPSILON {
            60.0 * (((bf - rf) / delta) + 2.0)
        } else {
            60.0 * (((rf - gf) / delta) + 4.0)
        };
        let hue = if hue < 0.0 { hue + 360.0 } else { hue };

        if (25.0..65.0).contains(&hue) {
            Self::Molten
        } else if (65.0..175.0).contains(&hue) {
            Self::Verdigris
        } else if (175.0..260.0).contains(&hue) {
            Self::Titanium
        } else {
            Self::Rust
        }
    }
}

/// Comprehensive color palette for Corten Desktop
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub border: Color,
    pub primary: Color,
    pub primary_hover: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

impl Palette {
    /// Corten Dark (Anthracite background, rust/copper accent, high-contrast cream text)
    pub const fn dark() -> Self {
        Self {
            background: Color::from_hex(0x141517),
            surface: Color::from_hex(0x1E2024),
            surface_hover: Color::from_hex(0x272B31),
            border: Color::from_hex(0x383D47), // Distinct border for card & box separation
            primary: Color::from_hex(0xD96B43), // Corten Rust / Copper
            primary_hover: Color::from_hex(0xEA7D56),
            text_primary: Color::from_hex(0xF4EFEA), // Very bright warm cream (> 12:1 contrast against bg)
            text_muted: Color::from_hex(0xA4ABB8),   // Crisp light gray (> 5.5:1 WCAG AA compliant)
            text_disabled: Color::from_hex(0x737A87), // Readable disabled text (> 3.5:1 against surface)
            success: Color::from_hex(0x5E9E68),
            warning: Color::from_hex(0xDE9E4B),
            danger: Color::from_hex(0xD75252),
        }
    }

    /// Corten Light (Warm stone/paper background, deep rust accent, dark text)
    pub const fn light() -> Self {
        Self {
            background: Color::from_hex(0xF4F1EA),
            surface: Color::from_hex(0xFFFFFF),
            surface_hover: Color::from_hex(0xEBE7DE),
            border: Color::from_hex(0xD0CBC0),
            primary: Color::from_hex(0xB85230), // Deep rust (≥ 4.5:1 with white text)
            primary_hover: Color::from_hex(0xA0451F),
            text_primary: Color::from_hex(0x1A1C1E),
            text_muted: Color::from_hex(0x5C616B),
            text_disabled: Color::from_hex(0x8D929D),
            success: Color::from_hex(0x427A4B),
            warning: Color::from_hex(0xB87B2E),
            danger: Color::from_hex(0xB83838),
        }
    }

    /// Corten High Contrast Dark (Pure OLED black background, pure white borders/text, high-visibility yellow/cyan)
    /// Exceeds WCAG AAA requirements with contrast ratio > 18:1.
    pub const fn high_contrast_dark() -> Self {
        Self {
            background: Color::from_hex(0x000000),
            surface: Color::from_hex(0x0A0A0C),
            surface_hover: Color::from_hex(0x18181C),
            border: Color::from_hex(0xFFFFFF),
            primary: Color::from_hex(0x00FFFF), // High-visibility bright cyan
            primary_hover: Color::from_hex(0x66FFFF),
            text_primary: Color::from_hex(0xFFFFFF), // 21:1 pure contrast
            text_muted: Color::from_hex(0xE6E6E6),   // > 16:1 contrast
            text_disabled: Color::from_hex(0xB0B0B0),
            success: Color::from_hex(0x00FF66),
            warning: Color::from_hex(0xFFFF00),
            danger: Color::from_hex(0xFF3333),
        }
    }

    /// Corten High Contrast Light (Pure white background, pure black borders/text, high-contrast navy)
    pub const fn high_contrast_light() -> Self {
        Self {
            background: Color::from_hex(0xFFFFFF),
            surface: Color::from_hex(0xF4F4F6),
            surface_hover: Color::from_hex(0xE4E4E8),
            border: Color::from_hex(0x000000),
            primary: Color::from_hex(0x0033CC),
            primary_hover: Color::from_hex(0x002299),
            text_primary: Color::from_hex(0x000000), // 21:1 pure contrast
            text_muted: Color::from_hex(0x1F1F1F),
            text_disabled: Color::from_hex(0x555555),
            success: Color::from_hex(0x006622),
            warning: Color::from_hex(0x885500),
            danger: Color::from_hex(0xCC0000),
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    /// Text / icon color for content placed on a filled `primary` surface.
    /// Picks white or anthracite ink — whichever gives the higher WCAG contrast —
    /// so every patina variant (incl. bright Molten / Titanium) stays legible.
    pub fn on_primary(&self) -> Color {
        let white = Color::from_hex(0xFFFFFF);
        let ink = Color::from_hex(0x141517);
        if white.contrast_ratio(&self.primary) >= ink.contrast_ratio(&self.primary) {
            white
        } else {
            ink
        }
    }

    /// Sunken "stage" surface used behind live previews / hero canvases.
    /// Darker than the background in dark themes, a soft stone tone in light themes.
    pub fn stage(&self) -> Color {
        if self.background.relative_luminance() < 0.5 {
            let b = self.background;
            Color::from_rgba(b.r * 0.72, b.g * 0.72, b.b * 0.72, 1.0)
        } else {
            self.surface_hover
        }
    }

    pub fn with_patina(mut self, patina: PatinaVariant) -> Self {
        let (prim, prim_hover) = patina.colors();
        self.primary = prim;
        self.primary_hover = prim_hover;
        self
    }

    /// Applies accessibility color vision deficiency filter across entire palette
    pub fn with_color_filter(mut self, filter: ColorFilterMode) -> Self {
        if filter == ColorFilterMode::None {
            return self;
        }
        self.background = self.background.with_color_filter(filter);
        self.surface = self.surface.with_color_filter(filter);
        self.surface_hover = self.surface_hover.with_color_filter(filter);
        self.border = self.border.with_color_filter(filter);
        self.primary = self.primary.with_color_filter(filter);
        self.primary_hover = self.primary_hover.with_color_filter(filter);
        self.text_primary = self.text_primary.with_color_filter(filter);
        self.text_muted = self.text_muted.with_color_filter(filter);
        self.text_disabled = self.text_disabled.with_color_filter(filter);
        self.success = self.success.with_color_filter(filter);
        self.warning = self.warning.with_color_filter(filter);
        self.danger = self.danger.with_color_filter(filter);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patina_variants_wcag() {
        for patina in PatinaVariant::ALL {
            let pal = Palette::dark().with_patina(patina);
            assert_eq!(patina.colors().0, pal.primary);
            assert!(!patina.name().is_empty());
            assert_eq!(PatinaVariant::from_code(patina.code()), patina);
        }

        // Test RGB hue matching
        assert_eq!(PatinaVariant::match_from_rgb(220, 100, 50), PatinaVariant::Rust);
        assert_eq!(PatinaVariant::match_from_rgb(50, 180, 140), PatinaVariant::Verdigris);
        assert_eq!(PatinaVariant::match_from_rgb(80, 120, 180), PatinaVariant::Titanium);
        assert_eq!(PatinaVariant::match_from_rgb(240, 170, 40), PatinaVariant::Molten);
        assert_eq!(PatinaVariant::match_from_rgb(100, 100, 100), PatinaVariant::Titanium);
    }

    #[test]
    fn test_on_primary_meets_wcag_aa_for_all_patinas() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let base = Palette::for_mode(mode);
            let mut palettes = vec![base];
            palettes.extend(PatinaVariant::ALL.iter().map(|p| base.with_patina(*p)));
            for pal in palettes {
                let ratio = pal.on_primary().contrast_ratio(&pal.primary);
                assert!(ratio >= 4.5, "{:?} primary {:?} on_primary contrast {:.2}", mode, pal.primary, ratio);
            }
        }
    }

    #[test]
    fn test_stage_contrasts_with_text() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let pal = Palette::for_mode(mode);
            assert!(pal.text_primary.contrast_ratio(&pal.stage()) >= 4.5);
            assert!(pal.text_muted.contrast_ratio(&pal.stage()) >= 3.0);
        }
    }

    #[test]
    fn test_wcag_contrast_ratios() {
        // WCAG AA requirement for normal text is at least 4.5:1
        // WCAG AA requirement for large text / UI components is at least 3.0:1

        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let palette = Palette::for_mode(mode);

            // Primary text against Background
            let bg_contrast = palette.text_primary.contrast_ratio(&palette.background);
            assert!(
                bg_contrast >= 4.5,
                "{:?} text_primary against background contrast was {:.2} (expected >= 4.5)",
                mode,
                bg_contrast
            );

            // Primary text against Surface
            let surface_contrast = palette.text_primary.contrast_ratio(&palette.surface);
            assert!(
                surface_contrast >= 4.5,
                "{:?} text_primary against surface contrast was {:.2} (expected >= 4.5)",
                mode,
                surface_contrast
            );

            // Muted text against Background
            let muted_contrast = palette.text_muted.contrast_ratio(&palette.background);
            assert!(
                muted_contrast >= 4.5,
                "{:?} text_muted against background contrast was {:.2} (expected >= 4.5)",
                mode,
                muted_contrast
            );
        }
    }

    #[test]
    fn test_high_contrast_wcag_aaa() {
        // High contrast dark mode: text_primary against background must exceed WCAG AAA (7.0:1)
        let hc_dark = Palette::high_contrast_dark();
        let dark_ratio = hc_dark.text_primary.contrast_ratio(&hc_dark.background);
        assert!(dark_ratio >= 15.0, "HC dark contrast ratio {dark_ratio:.1} must exceed 15.0");

        let dark_muted = hc_dark.text_muted.contrast_ratio(&hc_dark.background);
        assert!(dark_muted >= 7.0, "HC dark muted contrast {dark_muted:.1} must exceed WCAG AAA 7.0");

        // High contrast light mode: text_primary against background
        let hc_light = Palette::high_contrast_light();
        let light_ratio = hc_light.text_primary.contrast_ratio(&hc_light.background);
        assert!(light_ratio >= 15.0, "HC light contrast ratio {light_ratio:.1} must exceed 15.0");
    }

    #[test]
    fn test_color_vision_filters() {
        let original = Color::from_rgb(0.8, 0.2, 0.2); // Red dominant
        let protanopia = original.with_color_filter(ColorFilterMode::Protanopia);
        // Under protanopia, red component should shift down towards yellow/green mix
        assert!(protanopia.r < original.r);

        let mono = original.with_color_filter(ColorFilterMode::Monochrome);
        assert_eq!(mono.r, mono.g);
        assert_eq!(mono.g, mono.b);

        let pal = Palette::dark().with_color_filter(ColorFilterMode::Monochrome);
        assert_eq!(pal.primary.r, pal.primary.g);
        assert_eq!(pal.primary.g, pal.primary.b);
    }
}
