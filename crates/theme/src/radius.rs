/// Border radius tokens with default 12px for modern curves
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radius;

impl Radius {
    pub const NONE: f32 = 0.0;
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 12.0; // Varsayılan köşe yuvarlaklığı (12px)
    pub const LG: f32 = 16.0;
    pub const FULL: f32 = 9999.0; // Hap / Pill şekli
}
