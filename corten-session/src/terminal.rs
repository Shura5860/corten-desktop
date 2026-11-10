//! Corten Dedicated Terminal Wrapper
//! Ensures any terminal launched in Corten Desktop inherits the unified Corten Rust & Patina theme,
//! respects working directories, isolates config from user defaults, and prefers native Wayland terminals.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use corten_config::{AppearanceConfig, ConfigManager, ThemeModeChoice};
use corten_theme::{PatinaVariant, ThemeMode};

const KONSOLERC_CORTEN: &str = r#"[Desktop Entry]
DefaultProfile=Corten.profile
"#;

const CORTEN_PROFILE: &str = r#"[General]
Name=Corten
Parent=FALLBACK/

[Appearance]
ColorScheme=Corten
Font=Noto Sans Mono,11,-1,5,50,0,0,0,0,0
UseFontLineChararacters=true

[Scrolling]
HistoryMode=2
ScrollBarPosition=2

[Terminal Features]
BlinkingCursorEnabled=true
"#;

#[derive(Debug, Clone)]
pub struct TerminalPalette {
    pub bg_hex: String,
    pub fg_hex: String,
    pub bg_raw: String,
    pub fg_raw: String,
    pub bg_rgb: String,
    pub fg_rgb: String,
    pub prim_hex: String,
    pub prim_raw: String,
    pub prim_rgb: String,
    pub bright_hex: String,
    pub bright_raw: String,
    pub bright_rgb: String,
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self::from_config(PatinaVariant::Rust, ThemeMode::Dark)
    }
}

impl TerminalPalette {
    pub fn from_config(patina: PatinaVariant, mode: ThemeMode) -> Self {
        let (prim, sec) = patina.colors();

        let r1 = (prim.r * 255.0).round() as u8;
        let g1 = (prim.g * 255.0).round() as u8;
        let b1 = (prim.b * 255.0).round() as u8;

        let r2 = (sec.r * 255.0).round() as u8;
        let g2 = (sec.g * 255.0).round() as u8;
        let b2 = (sec.b * 255.0).round() as u8;

        let prim_hex = format!("#{:02x}{:02x}{:02x}", r1, g1, b1);
        let prim_raw = format!("{:02x}{:02x}{:02x}", r1, g1, b1);
        let prim_rgb = format!("{},{},{}", r1, g1, b1);

        let bright_hex = format!("#{:02x}{:02x}{:02x}", r2, g2, b2);
        let bright_raw = format!("{:02x}{:02x}{:02x}", r2, g2, b2);
        let bright_rgb = format!("{},{},{}", r2, g2, b2);

        let is_light = mode == ThemeMode::Light;
        let (bg_hex, fg_hex, bg_raw, fg_raw, bg_rgb, fg_rgb) = if is_light {
            ("#f5f6f8", "#1a1c23", "f5f6f8", "1a1c23", "245,246,248", "26,28,35")
        } else {
            ("#0f1012", "#f2eee9", "0f1012", "f2eee9", "15,16,18", "242,238,233")
        };

        Self {
            bg_hex: bg_hex.to_string(),
            fg_hex: fg_hex.to_string(),
            bg_raw: bg_raw.to_string(),
            fg_raw: fg_raw.to_string(),
            bg_rgb: bg_rgb.to_string(),
            fg_rgb: fg_rgb.to_string(),
            prim_hex,
            prim_raw,
            prim_rgb,
            bright_hex,
            bright_raw,
            bright_rgb,
        }
    }

    pub fn load_active() -> Self {
        let appearance_mgr = ConfigManager::<AppearanceConfig>::load_or_default("appearance").ok();
        let (patina, mode) = appearance_mgr
            .as_ref()
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let m = match c.get().theme_mode {
                    ThemeModeChoice::Light => ThemeMode::Light,
                    ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                (p, m)
            })
            .unwrap_or((PatinaVariant::Rust, ThemeMode::Dark));

        Self::from_config(patina, mode)
    }
}

pub fn generate_alacritty_toml(palette: &TerminalPalette) -> String {
    format!(
        r##"# Corten Desktop Terminal Theme for Alacritty (Dynamic Patina)
[window]
opacity = 0.92
blur = true
padding = {{ x = 12, y = 12 }}
title = "Corten Terminal"
class = {{ instance = "corten-terminal", general = "corten-terminal" }}

[font]
size = 11.0
normal = {{ family = "Noto Sans Mono", style = "Regular" }}
bold = {{ family = "Noto Sans Mono", style = "Bold" }}

[colors.primary]
background = "{bg_hex}"
foreground = "{fg_hex}"

[colors.cursor]
text = "{bg_hex}"
cursor = "{prim_hex}"

[colors.selection]
text = "#ffffff"
background = "#282b33"

[colors.normal]
black = "{bg_hex}"
red = "{prim_hex}"
green = "#45b09e"
yellow = "#e6a555"
blue = "#5288b8"
magenta = "#b8688e"
cyan = "#45b09e"
white = "{fg_hex}"

[colors.bright]
black = "#282b33"
red = "{bright_hex}"
green = "#5fc8b6"
yellow = "#f5ba6b"
blue = "#6da4d4"
magenta = "#d480a8"
cyan = "#73d2c0"
white = "#ffffff"
"##,
        bg_hex = palette.bg_hex,
        fg_hex = palette.fg_hex,
        prim_hex = palette.prim_hex,
        bright_hex = palette.bright_hex,
    )
}

pub fn generate_foot_ini(palette: &TerminalPalette) -> String {
    format!(
        r#"[main]
term=xterm-256color
font=Noto Sans Mono:size=11,monospace:size=11
pad=12x12
title=Corten Terminal
app-id=corten-terminal

[colors]
alpha=0.92
background={bg_raw}
foreground={fg_raw}

regular0={bg_raw}
regular1={prim_raw}
regular2=45b09e
regular3=e6a555
regular4=5288b8
regular5=b8688e
regular6=45b09e
regular7={fg_raw}

bright0=282b33
bright1={bright_raw}
bright2=5fc8b6
bright3=f5ba6b
bright4=6da4d4
bright5=d480a8
bright6=73d2c0
bright7=ffffff

[cursor]
style=beam
color={bg_raw} {prim_raw}
blink=yes
"#,
        bg_raw = palette.bg_raw,
        fg_raw = palette.fg_raw,
        prim_raw = palette.prim_raw,
        bright_raw = palette.bright_raw,
    )
}

pub fn generate_konsole_colorscheme(palette: &TerminalPalette) -> String {
    format!(
        r#"[General]
Description=Corten Rust & Patina
Opacity=0.92
Blur=true

[Background]
Color={bg_rgb}

[BackgroundFaint]
Color=27,29,34

[Foreground]
Color={fg_rgb}

[ForegroundIntense]
Color=255,255,255

[Color0]
Color={bg_rgb}

[Color0Intense]
Color=40,43,51

[Color1]
Color={prim_rgb}

[Color1Intense]
Color={bright_rgb}

[Color2]
Color=69,176,158

[Color2Intense]
Color=95,200,182

[Color3]
Color=230,165,85

[Color3Intense]
Color=245,185,115

[Color4]
Color=82,136,184

[Color4Intense]
Color=109,164,212

[Color5]
Color=184,104,142

[Color5Intense]
Color=212,128,168

[Color6]
Color=69,176,158

[Color6Intense]
Color=115,210,192

[Color7]
Color={fg_rgb}

[Color7Intense]
Color=255,255,255
"#,
        bg_rgb = palette.bg_rgb,
        fg_rgb = palette.fg_rgb,
        prim_rgb = palette.prim_rgb,
        bright_rgb = palette.bright_rgb,
    )
}

pub fn generate_kitty_conf(palette: &TerminalPalette) -> String {
    format!(
        r#"# Corten Desktop Terminal Theme for Kitty
background {bg_hex}
foreground {fg_hex}
background_opacity 0.92
background_blur 1
font_family Noto Sans Mono
font_size 11.0
window_padding_width 12

cursor {prim_hex}
cursor_text_color {bg_hex}
cursor_shape beam
cursor_blink_interval 0.5

color0 {bg_hex}
color1 {prim_hex}
color2 #45b09e
color3 #e6a555
color4 #5288b8
color5 #b8688e
color6 #45b09e
color7 {fg_hex}

color8 #282b33
color9 {bright_hex}
color10 #5fc8b6
color11 #f5ba6b
color12 #6da4d4
color13 #d480a8
color14 #73d2c0
color15 #ffffff
"#,
        bg_hex = palette.bg_hex,
        fg_hex = palette.fg_hex,
        prim_hex = palette.prim_hex,
        bright_hex = palette.bright_hex,
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TerminalCliArgs {
    pub working_dir: Option<PathBuf>,
    pub command: Option<Vec<String>>,
    pub show_help: bool,
    pub show_version: bool,
    pub passthrough_args: Vec<String>,
}

pub fn parse_cli_args<I, S>(args: I) -> TerminalCliArgs
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = TerminalCliArgs::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        let s = arg.as_ref();
        match s {
            "-h" | "--help" => {
                parsed.show_help = true;
            }
            "-v" | "--version" => {
                parsed.show_version = true;
            }
            "--working-directory" | "--workdir" | "-d" | "--directory" | "--dir" => {
                if let Some(val) = iter.next() {
                    parsed.working_dir = Some(PathBuf::from(val.as_ref()));
                }
            }
            s if s.starts_with("--working-directory=") => {
                if let Some((_, val)) = s.split_once('=') {
                    parsed.working_dir = Some(PathBuf::from(val));
                }
            }
            s if s.starts_with("--workdir=") => {
                if let Some((_, val)) = s.split_once('=') {
                    parsed.working_dir = Some(PathBuf::from(val));
                }
            }
            s if s.starts_with("--directory=") => {
                if let Some((_, val)) = s.split_once('=') {
                    parsed.working_dir = Some(PathBuf::from(val));
                }
            }
            "-e" => {
                let rest: Vec<String> = iter.map(|x| x.as_ref().to_string()).collect();
                if !rest.is_empty() {
                    parsed.command = Some(rest);
                }
                break;
            }
            other => {
                parsed.passthrough_args.push(other.to_string());
            }
        }
    }

    parsed
}

pub struct ConfigPaths {
    pub alacritty: PathBuf,
    pub foot: PathBuf,
    pub kitty: PathBuf,
}

fn ensure_corten_theme_files(palette: &TerminalPalette) -> Option<ConfigPaths> {
    let dirs = directories::BaseDirs::new()?;
    let home_path = dirs.home_dir();
    let config_dir = dirs.config_dir().join("corten").join("terminal");
    let _ = fs::create_dir_all(&config_dir);

    // 1. Isolated Corten Config Files
    let alacritty_path = config_dir.join("alacritty.toml");
    let _ = fs::write(&alacritty_path, generate_alacritty_toml(palette));

    let foot_path = config_dir.join("foot.ini");
    let _ = fs::write(&foot_path, generate_foot_ini(palette));

    let kitty_path = config_dir.join("kitty.conf");
    let _ = fs::write(&kitty_path, generate_kitty_conf(palette));

    // 2. Konsole profiles (non-destructive in dedicated named profile)
    let konsole_share = home_path.join(".local/share/konsole");
    let _ = fs::create_dir_all(&konsole_share);
    let _ = fs::write(
        konsole_share.join("Corten.colorscheme"),
        generate_konsole_colorscheme(palette),
    );
    let _ = fs::write(konsole_share.join("Corten.profile"), CORTEN_PROFILE);

    // 3. Flatpak Konsole
    let flatpak_konsole = home_path.join(".var/app/org.kde.konsole/data/konsole");
    if flatpak_konsole.exists() {
        let _ = fs::write(
            flatpak_konsole.join("Corten.colorscheme"),
            generate_konsole_colorscheme(palette),
        );
        let _ = fs::write(flatpak_konsole.join("Corten.profile"), CORTEN_PROFILE);
    }

    // 4. Default konsolerc only if not set
    let konsolerc_path = home_path.join(".config/konsolerc");
    if !konsolerc_path.exists() {
        let _ = fs::write(konsolerc_path, KONSOLERC_CORTEN);
    }

    Some(ConfigPaths {
        alacritty: alacritty_path,
        foot: foot_path,
        kitty: kitty_path,
    })
}

fn apply_common_cmd_settings(
    cmd: &mut Command,
    working_dir: &Option<PathBuf>,
    wayland_display: &str,
) {
    if let Some(ref dir) = working_dir {
        if dir.is_dir() {
            cmd.current_dir(dir);
        }
    }
    cmd.env("WAYLAND_DISPLAY", wayland_display);
    cmd.env("XDG_CURRENT_DESKTOP", "corten");
}

fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let cli_args = parse_cli_args(&raw_args);

    if cli_args.show_help {
        println!("Corten Terminal - Wayland Dedicated Terminal Wrapper");
        println!();
        println!("KULLANIM:");
        println!("    corten-terminal [SEÇENEKLER] [-e KOMUT [ARGÜMANLAR...]]");
        println!();
        println!("SEÇENEKLER:");
        println!("    --working-directory, --workdir, -d <DİZİN>   Başlangıç çalışma dizini");
        println!("    -e <KOMUT...>                               Terminal içinde çalıştırılacak komut");
        println!("    -h, --help                                  Bu yardım mesajını göster");
        println!("    -v, --version                               Sürüm bilgisini göster");
        println!();
        println!("DESTEKLENEN TERMİNALLER (Öncelik Sırasıyla):");
        println!("    alacritty, foot, kitty, konsole, pterm, xterm");
        return;
    }

    if cli_args.show_version {
        println!("corten-terminal 0.1.0");
        return;
    }

    let palette = TerminalPalette::load_active();
    let config_paths = ensure_corten_theme_files(&palette);

    let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "corten-0".into());

    // 0. User's explicit $TERMINAL environment override
    if let Ok(user_term) = std::env::var("TERMINAL") {
        if !user_term.trim().is_empty() {
            let mut cmd = Command::new(&user_term);
            apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
            if let Some(ref c) = cli_args.command {
                cmd.args(["-e"]).args(c);
            }
            cmd.args(&cli_args.passthrough_args);
            if cmd.spawn().is_ok() {
                return;
            }
        }
    }

    // 1. Alacritty (Native Wayland GPU accelerated Rust terminal)
    {
        let mut cmd = Command::new("alacritty");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["-T", "Corten Terminal", "--class", "corten-terminal,corten-terminal"]);

        if let Some(ref paths) = config_paths {
            if paths.alacritty.exists() {
                cmd.args(["--config-file", &paths.alacritty.to_string_lossy()]);
            }
        }

        if let Some(ref dir) = cli_args.working_dir {
            cmd.args(["--working-directory", &dir.to_string_lossy()]);
        }

        if let Some(ref c) = cli_args.command {
            cmd.arg("-e").args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 2. Foot (Native Wayland fast & lightweight terminal)
    {
        let mut cmd = Command::new("foot");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["-T", "Corten Terminal", "--app-id", "corten-terminal"]);

        if let Some(ref paths) = config_paths {
            if paths.foot.exists() {
                cmd.args(["-c", &paths.foot.to_string_lossy()]);
            }
        }

        if let Some(ref dir) = cli_args.working_dir {
            cmd.args(["-D", &dir.to_string_lossy()]);
        }

        if let Some(ref c) = cli_args.command {
            cmd.args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 3. Kitty (Native Wayland GPU accelerated terminal)
    {
        let mut cmd = Command::new("kitty");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["-T", "Corten Terminal", "--class", "corten-terminal"]);

        if let Some(ref paths) = config_paths {
            if paths.kitty.exists() {
                cmd.args(["--config", &paths.kitty.to_string_lossy()]);
            }
        }

        if let Some(ref dir) = cli_args.working_dir {
            cmd.args(["-d", &dir.to_string_lossy()]);
        }

        if let Some(ref c) = cli_args.command {
            cmd.args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 4. Native Konsole with Corten Profile
    {
        let mut cmd = Command::new("konsole");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["--profile", "Corten"]);

        if let Some(ref dir) = cli_args.working_dir {
            cmd.args(["--workdir", &dir.to_string_lossy()]);
        }

        if let Some(ref c) = cli_args.command {
            cmd.arg("-e").args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 5. Flatpak Konsole with Corten Profile
    let flatpak_bin = "/var/lib/flatpak/exports/bin/org.kde.konsole";
    if Path::new(flatpak_bin).exists() {
        let mut cmd = Command::new(flatpak_bin);
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["--profile", "Corten"]);

        if let Some(ref dir) = cli_args.working_dir {
            cmd.args(["--workdir", &dir.to_string_lossy()]);
        }

        if let Some(ref c) = cli_args.command {
            cmd.arg("-e").args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 6. PuTTY / pterm with Corten settings
    {
        let mut cmd = Command::new("pterm");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["-title", "Corten Terminal"]);
        if let Some(ref c) = cli_args.command {
            cmd.arg("-e").args(c);
        }
        cmd.args(&cli_args.passthrough_args);

        if cmd.spawn().is_ok() {
            return;
        }
    }

    // 7. Fallback: xterm
    {
        let mut cmd = Command::new("xterm");
        apply_common_cmd_settings(&mut cmd, &cli_args.working_dir, &wayland_display);
        cmd.args(["-title", "Corten Terminal"]);
        if let Some(ref c) = cli_args.command {
            cmd.arg("-e").args(c);
        }
        cmd.args(&cli_args.passthrough_args);
        let _ = cmd.spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args_empty() {
        let args: Vec<&str> = vec![];
        let parsed = parse_cli_args(args);
        assert_eq!(parsed.working_dir, None);
        assert_eq!(parsed.command, None);
        assert!(!parsed.show_help);
        assert!(!parsed.show_version);
        assert!(parsed.passthrough_args.is_empty());
    }

    #[test]
    fn test_parse_cli_args_working_directory_variants() {
        let parsed1 = parse_cli_args(["--working-directory", "/tmp"]);
        assert_eq!(parsed1.working_dir, Some(PathBuf::from("/tmp")));

        let parsed2 = parse_cli_args(["--workdir", "/var/log"]);
        assert_eq!(parsed2.working_dir, Some(PathBuf::from("/var/log")));

        let parsed3 = parse_cli_args(["-d", "/home/user"]);
        assert_eq!(parsed3.working_dir, Some(PathBuf::from("/home/user")));

        let parsed4 = parse_cli_args(["--working-directory=/etc"]);
        assert_eq!(parsed4.working_dir, Some(PathBuf::from("/etc")));

        let parsed5 = parse_cli_args(["--workdir=/usr/bin"]);
        assert_eq!(parsed5.working_dir, Some(PathBuf::from("/usr/bin")));
    }

    #[test]
    fn test_parse_cli_args_command_execution() {
        let parsed = parse_cli_args(["--working-directory", "/home", "-e", "htop", "--delay", "10"]);
        assert_eq!(parsed.working_dir, Some(PathBuf::from("/home")));
        assert_eq!(
            parsed.command,
            Some(vec!["htop".to_string(), "--delay".to_string(), "10".to_string()])
        );
    }

    #[test]
    fn test_parse_cli_args_help_and_version() {
        let parsed_h = parse_cli_args(["-h"]);
        assert!(parsed_h.show_help);

        let parsed_help = parse_cli_args(["--help"]);
        assert!(parsed_help.show_help);

        let parsed_v = parse_cli_args(["-v"]);
        assert!(parsed_v.show_version);

        let parsed_version = parse_cli_args(["--version"]);
        assert!(parsed_version.show_version);
    }

    #[test]
    fn test_terminal_palette_from_config() {
        let p_rust = TerminalPalette::from_config(PatinaVariant::Rust, ThemeMode::Dark);
        assert!(p_rust.prim_hex.starts_with('#'));
        assert_eq!(p_rust.bg_hex, "#0f1012");
        assert_eq!(p_rust.fg_hex, "#f2eee9");

        let p_verdigris = TerminalPalette::from_config(PatinaVariant::Verdigris, ThemeMode::Light);
        assert!(p_verdigris.prim_hex.starts_with('#'));
        assert_eq!(p_verdigris.bg_hex, "#f5f6f8");
        assert_eq!(p_verdigris.fg_hex, "#1a1c23");

        let p_titanium = TerminalPalette::from_config(PatinaVariant::Titanium, ThemeMode::Dark);
        assert!(p_titanium.prim_hex.starts_with('#'));

        let p_molten = TerminalPalette::from_config(PatinaVariant::Molten, ThemeMode::Dark);
        assert!(p_molten.prim_hex.starts_with('#'));
    }

    #[test]
    fn test_generated_configs_contain_palette_colors() {
        let palette = TerminalPalette::from_config(PatinaVariant::Rust, ThemeMode::Dark);

        let alacritty = generate_alacritty_toml(&palette);
        assert!(alacritty.contains(&palette.prim_hex));
        assert!(alacritty.contains(&palette.bg_hex));
        assert!(alacritty.contains("corten-terminal"));

        let foot = generate_foot_ini(&palette);
        assert!(foot.contains(&palette.prim_raw));
        assert!(foot.contains(&palette.bg_raw));
        assert!(foot.contains("corten-terminal"));

        let konsole = generate_konsole_colorscheme(&palette);
        assert!(konsole.contains(&palette.prim_rgb));
        assert!(konsole.contains(&palette.bg_rgb));

        let kitty = generate_kitty_conf(&palette);
        assert!(kitty.contains(&palette.prim_hex));
        assert!(kitty.contains(&palette.bg_hex));
    }
}
