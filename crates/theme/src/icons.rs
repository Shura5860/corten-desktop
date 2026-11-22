//! Lucide Icon Font bindings for Corten Desktop
//! Includes embedded font bytes and type-safe icon constants.

#[cfg(feature = "iced")]
pub const LUCIDE_FONT_BYTES: &[u8] = include_bytes!("../fonts/lucide.ttf");

#[cfg(feature = "iced")]
pub const LUCIDE_FONT: iced::Font = iced::Font::with_name("lucide");

#[cfg(feature = "iced")]
pub const NOTO_SANS_BYTES: &[u8] = include_bytes!("../fonts/noto_sans.ttf");

#[cfg(feature = "iced")]
pub const NOTO_SANS: iced::Font = iced::Font::with_name("Noto Sans");

#[cfg(feature = "iced")]
pub const NOTO_SANS_BOLD: iced::Font = iced::Font {
    family: iced::font::Family::Name("Noto Sans"),
    weight: iced::font::Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

#[cfg(feature = "iced")]
pub const NOTO_SANS_SEMIBOLD: iced::Font = iced::Font {
    family: iced::font::Family::Name("Noto Sans"),
    weight: iced::font::Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

#[cfg(feature = "iced")]
pub const NOTO_SANS_MEDIUM: iced::Font = iced::Font {
    family: iced::font::Family::Name("Noto Sans"),
    weight: iced::font::Weight::Normal,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

/// Standard Lucide icon character mappings
pub struct Icon;

impl Icon {
    pub const SUN: char = '\u{e178}';
    pub const MOON: char = '\u{e11e}';
    pub const MUSIC: char = '\u{e34c}'; // music-4
    pub const VOLUME: char = '\u{e1ab}'; // volume-2
    pub const CLOCK: char = '\u{e251}'; // clock-4
    pub const CALENDAR: char = '\u{e063}';
    pub const BELL: char = '\u{e059}';
    pub const TERMINAL: char = '\u{e181}';
    pub const SEARCH: char = '\u{e151}';
    pub const CLIPBOARD: char = '\u{e085}';
    pub const APP_WINDOW: char = '\u{e426}';
    pub const WIFI: char = '\u{e1ae}';
    pub const BATTERY: char = '\u{e053}';
    pub const POWER: char = '\u{e140}';
    pub const CODE: char = '\u{e093}';
    pub const FOLDER: char = '\u{e0d7}';
    pub const GLOBE: char = '\u{e0e8}';
    pub const SHIELD: char = '\u{e158}';
    pub const LOCK: char = '\u{e10b}';
    pub const USER: char = '\u{e19f}';
    pub const CHECK: char = '\u{e06c}';
    pub const X: char = '\u{e1b2}';
    pub const CHEVRON_RIGHT: char = '\u{e06f}';
    pub const KEYBOARD: char = '\u{e284}';
    pub const MONITOR: char = '\u{e11d}';
    pub const CPU: char = '\u{e0a9}';
    pub const SMILE: char = '\u{e0db}'; // face-slightly-frowning / smiley fallback
    pub const SETTINGS: char = '\u{e154}';
    pub const TRASH: char = '\u{e18e}'; // trash-2
    pub const MIC: char = '\u{e118}';
    pub const PIN: char = '\u{e259}';
    pub const DOWNLOAD: char = '\u{e0b2}';
    pub const UPLOAD: char = '\u{e19e}';
    pub const CAMERA: char = '\u{e064}';
    pub const CLOUD: char = '\u{e088}';
    pub const WIND: char = '\u{e1b0}';
    pub const DROPLET: char = '\u{e0b4}';
    pub const ZAP: char = '\u{e1b4}';
    pub const ACTIVITY: char = '\u{e038}';
    pub const COFFEE: char = '\u{e096}';
    pub const GAUGE: char = '\u{e1bf}';
    pub const EYE: char = '\u{e0ba}';
    pub const EYE_OFF: char = '\u{e0bb}';
    pub const VOLUME_X: char = '\u{e1ac}';
    pub const BELL_OFF: char = '\u{e05a}';
    pub const COPY: char = '\u{e09e}';
    pub const UNDO: char = '\u{e19b}';
    pub const REDO: char = '\u{e143}';
    pub const ARROW_UP_RIGHT: char = '\u{e04d}';
    pub const ARROW_DOWN: char = '\u{e042}';
    pub const ARROW_UP: char = '\u{e04a}';
    pub const BLUETOOTH: char = '\u{e05c}';
    pub const SQUARE: char = '\u{e167}';
    pub const PEN: char = '\u{e1f9}'; // pencil
    pub const FILE_TEXT: char = '\u{e0cc}';
    pub const LAYOUT_GRID: char = '\u{e0ff}';
    pub const LIST: char = '\u{e106}';
    pub const SPARKLES: char = '\u{e412}';
    pub const PALETTE: char = '\u{e1dd}';
    pub const GAMEPAD: char = '\u{e0df}'; // gamepad-2
    pub const ZOOM_IN: char = '\u{e1b6}';
    pub const ZOOM_OUT: char = '\u{e1b7}';
    pub const LAYERS: char = '\u{e52a}'; // layers-2
    pub const SLIDERS: char = '\u{e29a}'; // sliders-horizontal
    pub const PLAY: char = '\u{e13c}';
    pub const PAUSE: char = '\u{e12e}';
    pub const SKIP_BACK: char = '\u{e15f}';
    pub const SKIP_FORWARD: char = '\u{e160}';
    pub const SAVE: char = '\u{e14d}';
    pub const REFRESH: char = '\u{e145}'; // refresh-cw
    pub const SHUFFLE: char = '\u{e15e}';
}
