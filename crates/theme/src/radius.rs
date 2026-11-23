/// Corner radius tokens following a 4px geometric scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radius;

impl Radius {
    pub const NONE: f32 = 0.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 28.0;
    pub const FULL: f32 = 9999.0;
}
