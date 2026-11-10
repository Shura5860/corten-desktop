/// Layout tokens specifically tailored for desktop bars, panels, and docks
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarTokens;

impl BarTokens {
    /// Default height for floating & islands bars
    pub const HEIGHT: f32 = 32.0;

    /// Corner radius for floating bar containers (14px)
    pub const RADIUS_FLOATING: f32 = 14.0;

    /// Corner radius for module pill capsules
    pub const RADIUS_PILL: f32 = 9999.0;

    /// Screen margins for floating bar: (top: 8px, right: 12px, bottom: 0, left: 12px)
    pub const MARGIN_TOP: i32 = 8;
    pub const MARGIN_HORIZONTAL: i32 = 12;

    /// Default surface opacity for acrylic / frosted glass
    pub const SURFACE_ALPHA_FLOATING: f32 = 0.82;
    pub const SURFACE_ALPHA_PILL: f32 = 0.08;
    pub const SURFACE_ALPHA_PILL_HOVER: f32 = 0.16;
}
