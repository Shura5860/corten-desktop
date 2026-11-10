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

/// Standard Lucide icon character mappings
pub struct Icon;

impl Icon {
    pub const SUN: char = '\u{e178}';
    pub const MOON: char = '\u{e11e}';
    pub const MUSIC: char = '\u{e122}';
    pub const VOLUME: char = '\u{e1ab}';
    pub const CLOCK: char = '\u{e087}';
    pub const CALENDAR: char = '\u{e063}';
    pub const BELL: char = '\u{e059}';
    pub const TERMINAL: char = '\u{e181}';
    pub const SEARCH: char = '\u{e15c}';
    pub const CLIPBOARD: char = '\u{e084}';
    pub const APP_WINDOW: char = '\u{e04b}';
    pub const WIFI: char = '\u{e1b8}';
    pub const BATTERY: char = '\u{e053}';
    pub const POWER: char = '\u{e13c}';
    pub const CODE: char = '\u{e093}';
    pub const FOLDER: char = '\u{e0d7}';
    pub const GLOBE: char = '\u{e0e8}';
    pub const SHIELD: char = '\u{e1a0}';
    pub const LOCK: char = '\u{e48b}';
    pub const USER: char = '\u{e0c6}';
    pub const CHECK: char = '\u{e073}';
    pub const X: char = '\u{e1be}';
    pub const CHEVRON_RIGHT: char = '\u{e07a}';
    pub const KEYBOARD: char = '\u{e110}';
    pub const MONITOR: char = '\u{e120}';
    pub const CPU: char = '\u{e09c}';
    pub const SMILE: char = '\u{e175}';
    pub const SETTINGS: char = '\u{e167}';
    pub const TRASH: char = '\u{e1a1}';
    pub const MIC: char = '\u{e124}';
    pub const PIN: char = '\u{e26a}';
    pub const DOWNLOAD: char = '\u{e0b9}';
    pub const CAMERA: char = '\u{e067}';
    pub const CLOUD: char = '\u{e08c}';
    pub const WIND: char = '\u{e1c2}';
    pub const DROPLET: char = '\u{e0ba}';
    pub const ZAP: char = '\u{e1c5}';
    pub const ACTIVITY: char = '\u{e03b}';
    pub const COFFEE: char = '\u{e09b}';
    pub const GAUGE: char = '\u{e1d0}';
    pub const EYE: char = '\u{e0c4}';
    pub const EYE_OFF: char = '\u{e0c5}';
}
