/// Typography size and font family tokens
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typography;

impl Typography {
    pub const UI_FONT: &'static str = "Noto Sans";
    pub const MONO_FONT: &'static str = "JetBrains Mono";

    pub const OVERLINE: f32 = 11.0;
    pub const CAPTION: f32 = 11.0;
    pub const BODY_SM: f32 = 13.0;
    pub const BODY: f32 = 14.0;
    pub const TITLE_SM: f32 = 16.0;
    pub const TITLE: f32 = 20.0;
    pub const DISPLAY: f32 = 28.0;

    /// Scales a font size by an accessibility multiplier, clamped to reasonable bounds.
    pub fn scaled(size: f32, scale: f32) -> f32 {
        let scale = scale.clamp(1.0, 2.5);
        (size * scale).round()
    }
}
