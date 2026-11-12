use std::borrow::Cow;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tracing::{error, info};
use zbus::connection::Builder;
use zbus::interface;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_i18n::Language;
use corten_theme::iced_styles::input_style;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Transition, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES,
};
use iced::keyboard::key::Named;
use iced::keyboard::Key;
use iced::widget::{
    button, column, container, horizontal_space, image, mouse_area, row, scrollable, text, text_input,
    Space,
};
use iced::{event, Alignment, Element, Event, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

mod calc;
mod clipboard;
mod config;
mod entries;
mod files;
mod projects;

fn resolve_binary(name: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(name);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(cur) = std::env::current_dir() {
        let candidate = cur.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        let rel_cand = cur.join("target/release").join(name);
        if rel_cand.is_file() {
            return Some(rel_cand);
        }
        let target_cand = cur.join("target/debug").join(name);
        if target_cand.is_file() {
            return Some(target_cand);
        }
    }
    let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap_or(std::path::Path::new("."));
    let ws_rel = ws_root.join("target/release").join(name);
    if ws_rel.is_file() {
        return Some(ws_rel);
    }
    let ws_deb = ws_root.join("target/debug").join(name);
    if ws_deb.is_file() {
        return Some(ws_deb);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn launch_fallback_chain(spec: &str) -> std::io::Result<()> {
    let mut last_error = None;
    for candidate in spec.split("||") {
        let trimmed = candidate.trim();
        let mut argv = trimmed.split_whitespace();
        let Some(program) = argv.next() else { continue };

        // If candidate contains shell operators, execute via sh -c if binary exists
        if (trimmed.contains('>') || trimmed.contains('|') || trimmed.contains('&') || trimmed.contains('~'))
            && resolve_binary(program).is_some()
        {
            match Command::new("sh").arg("-c").arg(trimmed).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            }
        }

        if let Some(resolved) = resolve_binary(program) {
            match Command::new(resolved).args(argv).spawn() {
                Ok(_) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no executable candidate found")
    }))
}

fn app_icon_char(entry: &AppEntry) -> char {
    let lower_name = entry.name.to_lowercase();
    let lower_exec = entry.exec.to_lowercase();
    let lower_icon = entry.icon.as_deref().unwrap_or("").to_lowercase();
    let cat = entry.categories.join(" ").to_lowercase();
    let combined = format!("{} {} {} {}", lower_name, lower_exec, lower_icon, cat);

    if combined.contains("term")
        || combined.contains("alacritty")
        || combined.contains("kitty")
        || combined.contains("foot")
        || combined.contains("bash")
    {
        Icon::TERMINAL
    } else if combined.contains("firefox")
        || combined.contains("chrom")
        || combined.contains("brave")
        || combined.contains("zen")
        || combined.contains("browser")
        || combined.contains("web")
        || combined.contains("internet")
    {
        Icon::GLOBE
    } else if combined.contains("code")
        || combined.contains("rust")
        || combined.contains("nvim")
        || combined.contains("emacs")
        || combined.contains("vim")
        || combined.contains("ide")
        || combined.contains("develop")
    {
        Icon::CODE
    } else if combined.contains("file")
        || combined.contains("thunar")
        || combined.contains("nautilus")
        || combined.contains("dolphin")
        || combined.contains("corten-files")
        || combined.contains("manager")
    {
        Icon::FOLDER
    } else if combined.contains("setting")
        || combined.contains("corten-settings")
        || combined.contains("config")
        || combined.contains("control")
        || combined.contains("pref")
    {
        Icon::SETTINGS
    } else if combined.contains("music")
        || combined.contains("spotify")
        || combined.contains("vlc")
        || combined.contains("mpv")
        || combined.contains("audio")
        || combined.contains("sound")
    {
        Icon::MUSIC
    } else if combined.contains("image")
        || combined.contains("photo")
        || combined.contains("paint")
        || combined.contains("gimp")
        || combined.contains("inkscape")
        || combined.contains("draw")
    {
        Icon::APP_WINDOW
    } else if combined.contains("system")
        || combined.contains("monitor")
        || combined.contains("cpu")
        || combined.contains("task")
        || combined.contains("htop")
    {
        Icon::CPU
    } else if combined.contains("calc")
        || combined.contains("office")
        || combined.contains("writer")
        || combined.contains("document")
        || combined.contains("pdf")
        || combined.contains("note")
    {
        Icon::CLIPBOARD
    } else if combined.contains("lock")
        || combined.contains("key")
        || combined.contains("pass")
        || combined.contains("auth")
    {
        Icon::LOCK
    } else if combined.contains("mail")
        || combined.contains("thunderbird")
        || combined.contains("chat")
        || combined.contains("discord")
        || combined.contains("telegram")
    {
        Icon::BELL
    } else if combined.contains("ai") || combined.contains("corten-ai") || combined.contains("assistant") {
        Icon::CPU
    } else {
        Icon::APP_WINDOW
    }
}

fn render_app_icon<'a, M: 'a>(
    entry: &AppEntry,
    size: f32,
    primary: iced::Color,
) -> Element<'a, M> {
    if let Some(ref icon_path) = entry.icon_path {
        iced::widget::image(image::Handle::from_path(icon_path))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into()
    } else {
        let sym = app_icon_char(entry);
        container(
            text(sym)
                .font(LUCIDE_FONT)
                .size(size * 0.55)
                .color(primary),
        )
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                primary.r,
                primary.g,
                primary.b,
                0.14,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(
                    primary.r,
                    primary.g,
                    primary.b,
                    0.30,
                ),
                width: 1.0,
                radius: iced::border::Radius::from(size * 0.25),
            },
            ..Default::default()
        })
        .into()
    }
}

fn render_smart_empty_state<'a>(query: &'a str, palette: Palette) -> Element<'a, Message> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return container(
            column![
                text("Eşleşen uygulama veya dosya bulunamadı")
                    .size(Typography::BODY)
                    .color(palette.text_muted.to_iced()),
                text("Başka bir arama terimi deneyin veya kurulu uygulamaları kontrol edin")
                    .size(Typography::CAPTION)
                    .color(palette.border.to_iced()),
            ]
            .spacing(Spacing::XXS)
            .align_x(Alignment::Center),
        )
        .padding(Spacing::LG)
        .center_x(Length::Fill)
        .into();
    }

    let q_term = clean_query.to_string();
    let q_web = clean_query.to_string();
    let q_files = clean_query.to_string();

    let term_btn = button(
        row![
            container(text(Icon::TERMINAL).font(LUCIDE_FONT).size(16.0).color(palette.primary.to_iced()))
                .width(Length::Fixed(24.0))
                .center_x(Length::Fill),
            column![
                row![
                    text("Terminalde Çalıştır: ").size(12.0).color(palette.text_primary.to_iced()),
                    text(clean_query).size(12.0).font(iced::Font::MONOSPACE).color(palette.primary.to_iced()),
                ].spacing(4),
                text("Komutu terminal konsolunda yürütür").size(10.0).color(palette.text_muted.to_iced()),
            ].spacing(2).width(Length::Fill),
            container(text("[Enter]").size(10.0).color(palette.primary.to_iced()))
                .padding([2, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                        width: 1.0,
                        radius: iced::border::Radius::from(4.0),
                    },
                    ..Default::default()
                }),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    )
    .width(Length::Fill)
    .padding([8, 12])
    .style(move |_, status| {
        let bg = match status {
            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            _ => iced::Color::from_rgba(palette.surface.r, palette.surface.g, palette.surface.b, 0.8),
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        }
    })
    .on_press(Message::LaunchCommand(format!("corten-terminal -e {} || alacritty -e {} || foot {} || kitty {} || xterm -e {}", q_term, q_term, q_term, q_term, q_term)));

    let web_btn = button(
        row![
            container(text(Icon::GLOBE).font(LUCIDE_FONT).size(16.0).color(palette.primary.to_iced()))
                .width(Length::Fixed(24.0))
                .center_x(Length::Fill),
            column![
                row![
                    text("Web'de Ara: ").size(12.0).color(palette.text_primary.to_iced()),
                    text(clean_query).size(12.0).font(iced::Font::MONOSPACE).color(palette.primary.to_iced()),
                ].spacing(4),
                text("Varsayılan tarayıcıda arama yapar").size(10.0).color(palette.text_muted.to_iced()),
            ].spacing(2).width(Length::Fill),
            container(text("[Ctrl+Enter]").size(10.0).color(palette.primary.to_iced()))
                .padding([2, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                        width: 1.0,
                        radius: iced::border::Radius::from(4.0),
                    },
                    ..Default::default()
                }),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    )
    .width(Length::Fill)
    .padding([8, 12])
    .style(move |_, status| {
        let bg = match status {
            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            _ => iced::Color::from_rgba(palette.surface.r, palette.surface.g, palette.surface.b, 0.8),
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.border.r, palette.border.g, palette.border.b, 0.6),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        }
    })
    .on_press(Message::LaunchCommand(format!("xdg-open 'https://duckduckgo.com/?q={}'", q_web)));

    let files_btn = button(
        row![
            container(text(Icon::FOLDER).font(LUCIDE_FONT).size(16.0).color(palette.primary.to_iced()))
                .width(Length::Fixed(24.0))
                .center_x(Length::Fill),
            column![
                row![
                    text("Dosyalarda Ara: ").size(12.0).color(palette.text_primary.to_iced()),
                    text(clean_query).size(12.0).font(iced::Font::MONOSPACE).color(palette.primary.to_iced()),
                ].spacing(4),
                text("Corten Files dosya yöneticisinde aratır").size(10.0).color(palette.text_muted.to_iced()),
            ].spacing(2).width(Length::Fill),
            container(text("[Alt+Enter]").size(10.0).color(palette.primary.to_iced()))
                .padding([2, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                        width: 1.0,
                        radius: iced::border::Radius::from(4.0),
                    },
                    ..Default::default()
                }),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    )
    .width(Length::Fill)
    .padding([8, 12])
    .style(move |_, status| {
        let bg = match status {
            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            _ => iced::Color::from_rgba(palette.surface.r, palette.surface.g, palette.surface.b, 0.8),
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.border.r, palette.border.g, palette.border.b, 0.6),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        }
    })
    .on_press(Message::LaunchCommand(format!("corten-files --search '{}'", q_files)));

    container(
        column![
            row![
                container(text(Icon::SEARCH).font(LUCIDE_FONT).size(14.0).color(palette.primary.to_iced()))
                    .padding([2, 6])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.18))),
                        border: iced::Border {
                            radius: iced::border::Radius::from(4.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                text("Eşleşen uygulama bulunamadı").size(13.0).color(palette.text_primary.to_iced()),
            ].spacing(8).align_y(Alignment::Center),
            term_btn,
            web_btn,
            files_btn,
        ]
        .spacing(8)
        .width(Length::Fill)
    )
    .padding(Spacing::MD)
    .width(Length::Fill)
    .into()
}

fn render_command_list<'a>(commands: &'a [QuickCommand], selected_index: usize, palette: Palette) -> Element<'a, Message> {
    let mut list = column![].spacing(4).width(Length::Fill);
    for (idx, cmd) in commands.iter().enumerate() {
        let is_sel = idx == selected_index;
        let cmd_exec = cmd.command.to_string();

        let active_strip = container(text(""))
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(20.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(if is_sel {
                    palette.primary.to_iced()
                } else {
                    iced::Color::TRANSPARENT
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(2.0),
                    ..Default::default()
                },
                ..Default::default()
            });

        let (bg, border_color) = if is_sel {
            (palette.surface_hover.to_iced(), palette.primary.to_iced())
        } else {
            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
        };

        let icon_box = container(
            text(cmd.icon_char)
                .font(LUCIDE_FONT)
                .size(18.0)
                .color(palette.primary.to_iced()),
        )
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .center_x(Length::Fill)
        .center_y(Length::Fill);

        let item = button(
            row![
                active_strip,
                icon_box,
                column![
                    text(format!("> {}", cmd.name))
                        .size(13.0)
                        .font(iced::Font {
                            weight: iced::font::Weight::Semibold,
                            ..Default::default()
                        })
                        .color(palette.text_primary.to_iced()),
                    text(cmd.description)
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(2.0)
                .width(Length::Fill),
                text("Sistem Komutu")
                    .size(10.0)
                    .color(palette.primary.to_iced()),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .padding([6.0, 10.0])
        .width(Length::Fill)
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: palette.text_primary.to_iced(),
            border: iced::Border {
                color: border_color,
                width: if is_sel { 1.0 } else { 0.0 },
                radius: iced::border::Radius::from(Radius::SM),
            },
            shadow: iced::Shadow::default(),
        })
        .on_press(Message::LaunchCommand(cmd_exec));

        list = list.push(item);
    }
    list.into()
}

use calc::{eval_math_expression, format_calc_result, parse_color_query};
use clipboard::{ClipboardEntry, ClipboardManager};
use config::{LauncherConfig, LauncherStyle};
use entries::{scan_desktop_entries, AppEntry};
use files::{find_matching_files, LauncherFileEntry};
use projects::{scan_dev_projects, DevProject};

const SCROLLABLE_ID: &str = "launcher-results-scrollable";

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting Corten Application Launcher...");

    let config_manager = ConfigManager::<LauncherConfig>::load_or_default("launcher")?;
    let initial_config = config_manager.get().clone();

    let (ipc_tx, ipc_rx) = unbounded_channel();

    // Spawn D-Bus org.corten.Launcher service for CLI / shortcut toggling
    let dbus_tx = ipc_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let server = LauncherServer { tx: dbus_tx };
            if let Ok(b) = Builder::session() {
                if let Ok(b_named) = b.name("org.corten.Launcher") {
                    if let Ok(b_served) = b_named.serve_at("/org/corten/Launcher", server) {
                        if let Ok(_conn) = b_served.build().await {
                            std::future::pending::<()>().await;
                        }
                    }
                }
            }
        });
    });

    let args: Vec<String> = std::env::args().collect();
    let stay_open = args
        .iter()
        .any(|arg| arg == "--no-exit" || arg == "--test" || arg == "-t");
    let is_dashboard_arg = args.iter().any(|arg| arg == "--dashboard" || arg == "-d");
    let is_start_menu_arg = args.iter().any(|arg| arg == "--start-menu" || arg == "-m");
    let is_emoji_arg = args.iter().any(|arg| arg == "--emoji" || arg == "-e");
    let is_notes_arg = args.iter().any(|arg| arg == "--notes" || arg == "-n");
    let is_cheat_sheet_arg = args
        .iter()
        .any(|arg| arg == "--cheat-sheet" || arg == "-c" || arg == "--shortcuts");
    let is_desktop_menu_arg = args
        .iter()
        .any(|arg| arg == "--desktop-menu" || arg == "--context-menu");
    let cursor_x: Option<i32> = args
        .windows(2)
        .find(|w| w[0] == "--x" || w[0] == "-x")
        .and_then(|w| w[1].parse().ok());
    let cursor_y: Option<i32> = args
        .windows(2)
        .find(|w| w[0] == "--y" || w[0] == "-y")
        .and_then(|w| w[1].parse().ok());

    let initial_style = if is_start_menu_arg {
        LauncherStyle::StartMenu
    } else if is_dashboard_arg {
        LauncherStyle::Dashboard
    } else {
        initial_config.style
    };

    let initial_mode = if is_desktop_menu_arg {
        LauncherMode::DesktopMenu
    } else if is_cheat_sheet_arg {
        LauncherMode::CheatSheet
    } else if is_emoji_arg {
        LauncherMode::Emoji
    } else if is_notes_arg {
        LauncherMode::Notes
    } else {
        LauncherMode::Applications
    };

    let flags = LauncherAppFlags {
        config_manager,
        ipc_rx: Some(ipc_rx),
        stay_open,
        initial_style,
        initial_mode,
        cursor_pos: (cursor_x, cursor_y),
    };

    let (initial_margin, anchor, layer_size) =
        ((0, 0, 0, 0), Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right, Some((0, 0)));

    CortenLauncherApp::run(Settings {
        id: Some("corten-launcher".into()),
        flags,
        fonts: vec![Cow::Borrowed(LUCIDE_FONT_BYTES), Cow::Borrowed(NOTO_SANS_BYTES)],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Overlay,
            margin: initial_margin,
            exclusive_zone: -1,
            size: layer_size,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(16.0),
        antialiasing: false,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

pub struct LauncherServer {
    tx: tokio::sync::mpsc::UnboundedSender<LauncherIpcCommand>,
}

#[derive(Debug, Clone)]
pub enum LauncherIpcCommand {
    Toggle,
    Open,
    Close,
    ReloadConfig,
}

#[interface(name = "org.corten.Launcher")]
impl LauncherServer {
    async fn toggle(&self) {
        let _ = self.tx.send(LauncherIpcCommand::Toggle);
    }
    async fn open(&self) {
        let _ = self.tx.send(LauncherIpcCommand::Open);
    }
    async fn close(&self) {
        let _ = self.tx.send(LauncherIpcCommand::Close);
    }
    async fn reload_config(&self) {
        let _ = self.tx.send(LauncherIpcCommand::ReloadConfig);
    }
}

pub struct LauncherAppFlags {
    pub config_manager: ConfigManager<LauncherConfig>,
    pub ipc_rx: Option<UnboundedReceiver<LauncherIpcCommand>>,
    pub stay_open: bool,
    pub initial_style: LauncherStyle,
    pub initial_mode: LauncherMode,
    pub cursor_pos: (Option<i32>, Option<i32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    All,
    Internet,
    Office,
    Media,
    Development,
    System,
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchInputChanged(String),
    SelectPrevious,
    SelectNext,
    ExecuteSelected,
    LaunchSpecificApp(AppEntry),
    LaunchSpecificProject(DevProject),
    OpenProjectTerminal(std::path::PathBuf),
    OpenProjectFiles(std::path::PathBuf),
    LaunchSpecificFile(LauncherFileEntry),
    CopyCalcResult(String),
    CopySpecificClipboard(ClipboardEntry),
    SelectEmoji(String),
    SelectDesktopAction(DesktopAction),
    SelectCategory(AppCategory),
    SwitchMode(LauncherMode),
    ToggleMode,
    ToggleLauncherStyle,
    SwitchLauncherStyle(LauncherStyle),
    OpenSettings,
    LockScreen,
    PowerOff,
    CloseRequested,
    NoOp,
    LaunchCommand(String),
    IpcCommand(LauncherIpcCommand),
    IcedEvent(Event),
    AnimationTick,
    LayerAction(LayershellCustomActions),
}

impl TryInto<LayershellCustomActions> for Message {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            Message::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherMode {
    Applications,
    Projects,
    Clipboard,
    Emoji,
    Notes,
    CheatSheet,
    DesktopMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAction {
    NewFolder,
    Terminal,
    FileManager,
    NextWallpaper,
    ToggleWidgets,
    WallpaperAppearance,
    CheatSheet,
    Settings,
    PowerHub,
}

#[derive(Debug, Clone)]
pub struct DesktopMenuItem {
    pub icon_char: char,
    pub title: &'static str,
    pub shortcut: &'static str,
    pub action: DesktopAction,
}

#[derive(Debug, Clone)]
pub struct ShortcutEntry {
    pub category: &'static str,
    pub chords: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
    pub tip: &'static str,
    pub icon_char: char,
}

#[derive(Debug, Clone)]
pub struct QuickCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub icon_char: char,
    pub command: &'static str,
}

pub static QUICK_COMMANDS: &[QuickCommand] = &[
    QuickCommand {
        name: "lock",
        description: "Ekranı kilitle (Corten Lock)",
        icon_char: Icon::LOCK,
        command: "corten-lock",
    },
    QuickCommand {
        name: "sleep",
        description: "Sistemi askıya al (Suspend)",
        icon_char: Icon::MOON,
        command: "systemctl suspend",
    },
    QuickCommand {
        name: "screenshot",
        description: "Ekran alıntısı aracı (Corten Snipper)",
        icon_char: Icon::CAMERA,
        command: "corten-snipper",
    },
    QuickCommand {
        name: "settings",
        description: "Sistem Ayarları (Corten Settings)",
        icon_char: Icon::SETTINGS,
        command: "corten-settings",
    },
    QuickCommand {
        name: "files",
        description: "Dosya Yöneticisi (Corten Files)",
        icon_char: Icon::FOLDER,
        command: "corten-files",
    },
    QuickCommand {
        name: "terminal",
        description: "Terminal Öykünücüsü",
        icon_char: Icon::TERMINAL,
        command: "corten-terminal || alacritty || foot || kitty || xterm",
    },
    QuickCommand {
        name: "cheatsheet",
        description: "Klavye Kısayolları Rehberi",
        icon_char: Icon::KEYBOARD,
        command: "corten-launcher --cheat-sheet",
    },
    QuickCommand {
        name: "reboot",
        description: "Sistemi Yeniden Başlat",
        icon_char: Icon::ZAP,
        command: "systemctl reboot",
    },
    QuickCommand {
        name: "shutdown",
        description: "Bilgisayarı Kapat",
        icon_char: Icon::POWER,
        command: "systemctl poweroff",
    },
];

#[derive(Debug, Clone)]
pub struct EmojiEntry {
    pub emoji: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub keywords: &'static str,
}

pub static EMOJI_DATABASE: &[EmojiEntry] = &[
    EmojiEntry { emoji: "😀", name: "Sırıtan Yüz", category: "İfadeler", keywords: "smile grinning happy sırıt mutlu" },
    EmojiEntry { emoji: "😃", name: "Gülen Gözler", category: "İfadeler", keywords: "smiley happy neşeli sevinç" },
    EmojiEntry { emoji: "😄", name: "Mutlu Yüz", category: "İfadeler", keywords: "smile laugh kahkaha" },
    EmojiEntry { emoji: "😂", name: "Gözünden Yaş Gelen Gülen Yüz", category: "İfadeler", keywords: "joy tears laugh komik ağlama gülmek rofl lol" },
    EmojiEntry { emoji: "🤣", name: "Gülmekten Kırılan Yüz", category: "İfadeler", keywords: "rofl rolling on the floor laughing komik" },
    EmojiEntry { emoji: "😊", name: "Tebessüm", category: "İfadeler", keywords: "blush warm tebessüm tatlı sevimli" },
    EmojiEntry { emoji: "😍", name: "Kalp Gözlü Yüz", category: "İfadeler", keywords: "heart eyes love aşk sevgi aşık" },
    EmojiEntry { emoji: "🥰", name: "Kalpli Yüz", category: "İfadeler", keywords: "smiling hearts love romantik şefkat" },
    EmojiEntry { emoji: "😎", name: "Güneş Gözlüklü Yüz", category: "İfadeler", keywords: "cool sunglasses havalı tarz karizma" },
    EmojiEntry { emoji: "🤔", name: "Düşünen Yüz", category: "İfadeler", keywords: "thinking ponder düşünce soru merak" },
    EmojiEntry { emoji: "🫡", name: "Asker Selamı", category: "İfadeler", keywords: "salute saygı emret tamamdır" },
    EmojiEntry { emoji: "🤫", name: "Sessiz Ol", category: "İfadeler", keywords: "shush quiet secret sır sessiz" },
    EmojiEntry { emoji: "🥳", name: "Parti Yüzü", category: "İfadeler", keywords: "party celebrate kutlama tebrik doğumgünü" },
    EmojiEntry { emoji: "😴", name: "Uyuyan Yüz", category: "İfadeler", keywords: "sleeping zzz uyku yorgun" },
    EmojiEntry { emoji: "😭", name: "Hüngür Ağlayan Yüz", category: "İfadeler", keywords: "crying tears üzgün hüzün cry" },
    EmojiEntry { emoji: "😱", name: "Çığlık / Şaşkın Yüz", category: "İfadeler", keywords: "screaming fear korku şaşkınlık panik shock" },
    EmojiEntry { emoji: "👍", name: "Başparmak Yukarı", category: "Hareketler", keywords: "thumbs up like onay peki süper evet good" },
    EmojiEntry { emoji: "👎", name: "Başparmak Aşağı", category: "Hareketler", keywords: "thumbs down dislike hayır kötü red" },
    EmojiEntry { emoji: "👏", name: "Alkış", category: "Hareketler", keywords: "applause clap alkış tebrik bravo" },
    EmojiEntry { emoji: "🙌", name: "Eller Havaya", category: "Hareketler", keywords: "raising hands celebration yaşasın şükür" },
    EmojiEntry { emoji: "🤝", name: "El Sıkışma", category: "Hareketler", keywords: "handshake agreement anlaşma tanışma ortaklık" },
    EmojiEntry { emoji: "✌️", name: "Barış / Zafer İşareti", category: "Hareketler", keywords: "peace victory zafer barış" },
    EmojiEntry { emoji: "🤞", name: "Şans Dileği", category: "Hareketler", keywords: "fingers crossed luck şans inşallah" },
    EmojiEntry { emoji: "👊", name: "Yumruk Çakma", category: "Hareketler", keywords: "fist bump güç bro selam" },
    EmojiEntry { emoji: "🙏", name: "Dua / Rica", category: "Hareketler", keywords: "please pray folded hands rica dua lütfen teşekkür" },
    EmojiEntry { emoji: "👋", name: "El Sallama", category: "Hareketler", keywords: "wave waving hello bye merhaba hoşçakal selam" },
    EmojiEntry { emoji: "❤️", name: "Kırmızı Kalp", category: "Semboller", keywords: "red heart love aşk sevgi kalp" },
    EmojiEntry { emoji: "💖", name: "Işıldayan Kalp", category: "Semboller", keywords: "sparkling heart aşk parıltı" },
    EmojiEntry { emoji: "💔", name: "Kırık Kalp", category: "Semboller", keywords: "broken heart üzüntü ayrılık heartbreak" },
    EmojiEntry { emoji: "🔥", name: "Ateş", category: "Semboller", keywords: "fire flame lit alev sıcak yakıcı popüler" },
    EmojiEntry { emoji: "✨", name: "Parıltılar", category: "Semboller", keywords: "sparkles shiny sihir parıltı parlak" },
    EmojiEntry { emoji: "⭐", name: "Yıldız", category: "Semboller", keywords: "star favorite yıldız favori" },
    EmojiEntry { emoji: "💯", name: "Yüz Puan", category: "Semboller", keywords: "hundred points mükemmel tam puan perfect 100" },
    EmojiEntry { emoji: "🎉", name: "Parti Konfetisi", category: "Semboller", keywords: "tada party celebration tebrikler kutlama" },
    EmojiEntry { emoji: "🚀", name: "Roket", category: "Teknoloji", keywords: "rocket fast hız yükseliş uzay deploy fırlatma" },
    EmojiEntry { emoji: "🦀", name: "Rust Yengeci (Ferris)", category: "Teknoloji", keywords: "rust crab ferris yengeç cargo" },
    EmojiEntry { emoji: "💻", name: "Dizüstü Bilgisayar", category: "Teknoloji", keywords: "laptop computer pc kod yazılım laptop bilgisayar" },
    EmojiEntry { emoji: "🖥️", name: "Masaüstü Ekran", category: "Teknoloji", keywords: "desktop monitor ekran display" },
    EmojiEntry { emoji: "⚙️", name: "Dişli / Ayarlar", category: "Teknoloji", keywords: "gear settings config ayarlar yapılandırma" },
    EmojiEntry { emoji: "🛠️", name: "Alet Çantası", category: "Teknoloji", keywords: "tools hammer wrench tamir araç geliştirme" },
    EmojiEntry { emoji: "📦", name: "Paket", category: "Teknoloji", keywords: "package box crate kargo modül" },
    EmojiEntry { emoji: "🐛", name: "Böcek (Bug)", category: "Teknoloji", keywords: "bug error hata hata ayıklama debug" },
    EmojiEntry { emoji: "🔒", name: "Kilit / Güvenlik", category: "Teknoloji", keywords: "lock secure security kilit gizli" },
    EmojiEntry { emoji: "🔑", name: "Anahtar", category: "Teknoloji", keywords: "key password access anahtar token" },
    EmojiEntry { emoji: "☕", name: "Kahve", category: "Nesneler", keywords: "coffee tea cafe mola dinlenme" },
    EmojiEntry { emoji: "💡", name: "Ampul / Fikir", category: "Nesneler", keywords: "idea bulb light ampul fikir ilham" },
    EmojiEntry { emoji: "🎯", name: "Hedef", category: "Nesneler", keywords: "target goal hit isabet amaç" },
    EmojiEntry { emoji: "📌", name: "Raptiye", category: "Nesneler", keywords: "pin pushpin sabitle iliştir not" },
    EmojiEntry { emoji: "📝", name: "Not / Belge", category: "Nesneler", keywords: "memo note document yazı metin not" },
    EmojiEntry { emoji: "🌐", name: "Dünya / Ağ", category: "Teknoloji", keywords: "globe internet network web dünya ağ" },
    EmojiEntry { emoji: "⚡", name: "Şimşek / Yıldırım", category: "Semboller", keywords: "zap lightning bolt şimşek elektrik hızlı" },
    EmojiEntry { emoji: "✔️", name: "Onay İşareti", category: "Semboller", keywords: "check checkmark done tamam yapıldı onay" },
    EmojiEntry { emoji: "❌", name: "Çarpı İşareti", category: "Semboller", keywords: "cross cancel iptal hayır hata" },
    EmojiEntry { emoji: "⚠️", name: "Uyarı İşareti", category: "Semboller", keywords: "warning caution dikkat ikaz tehlike" },
    EmojiEntry { emoji: "🟢", name: "Yeşil Daire", category: "Semboller", keywords: "green circle online aktif çevrimiçi" },
    EmojiEntry { emoji: "🔴", name: "Kırmızı Daire", category: "Semboller", keywords: "red circle offline kapalı dur" },
];

pub struct CortenLauncherApp {
    config: LauncherConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
    mode: LauncherMode,
    cursor_pos: (Option<i32>, Option<i32>),
    selected_category: AppCategory,
    all_entries: Vec<AppEntry>,
    all_projects: Vec<DevProject>,
    clipboard_entries: Vec<ClipboardEntry>,
    search_query: String,
    filtered_entries: Vec<AppEntry>,
    filtered_projects: Vec<DevProject>,
    filtered_clipboard: Vec<ClipboardEntry>,
    filtered_emojis: Vec<EmojiEntry>,
    notes_entries: Vec<String>,
    filtered_notes: Vec<String>,
    filtered_files: Vec<LauncherFileEntry>,
    filtered_commands: Vec<QuickCommand>,
    calc_result: Option<(String, String)>,
    selected_index: usize,
    matcher: nucleo::Matcher,
    ipc_rx: Option<Arc<tokio::sync::Mutex<UnboundedReceiver<LauncherIpcCommand>>>>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    language: Language,
    is_visible: bool,
    stay_open: bool,
    should_exit: bool,
    status_message: Option<String>,
    entrance: Transition,
    mode_transition: Transition,
}

impl Application for CortenLauncherApp {
    type Message = Message;
    type Flags = LauncherAppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(mut flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let mut config = flags.config_manager.get().clone();
        config.style = flags.initial_style;
        let watcher = flags.config_manager.spawn_watcher().ok().map(|(w, _)| w);
        let ipc_rx = flags
            .ipc_rx
            .take()
            .map(|rx| Arc::new(tokio::sync::Mutex::new(rx)));

        let all_entries = scan_desktop_entries();
        let initial_limit = if config.style == LauncherStyle::Dashboard { 200 } else if config.style == LauncherStyle::StartMenu { 120 } else { config.max_results };
        let filtered_entries = all_entries
            .iter()
            .take(initial_limit)
            .cloned()
            .collect();

        let all_projects = scan_dev_projects();
        let filtered_projects = all_projects
            .iter()
            .take(config.max_results)
            .cloned()
            .collect();
        let matcher = nucleo::Matcher::default();

        let load_font_task =
            iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::SearchInputChanged(String::new()));

        let stay_open = flags.stay_open;
        let (language, active_patina, theme_mode) = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let pat = PatinaVariant::from_code(&c.get().patina);
                let lang = Language::from_code(&c.get().language);
                let mode = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    _ => ThemeMode::Dark,
                };
                (lang, pat, mode)
            })
            .unwrap_or_else(|_| (Language::detect_system(), PatinaVariant::Rust, ThemeMode::Dark));

        (
            Self {
                config,
                _config_watcher: watcher,
                mode: flags.initial_mode,
                cursor_pos: flags.cursor_pos,
                selected_category: AppCategory::All,
                all_entries,
                all_projects,
                clipboard_entries: Vec::new(),
                search_query: String::new(),
                filtered_entries,
                filtered_projects,
                filtered_clipboard: Vec::new(),
                filtered_emojis: EMOJI_DATABASE.to_vec(),
                notes_entries: Vec::new(),
                filtered_notes: Vec::new(),
                filtered_files: Vec::new(),
                filtered_commands: Vec::new(),
                calc_result: None,
                selected_index: 0,
                matcher,
                ipc_rx,
                theme_mode,
                active_patina,
                language,
                is_visible: true,
                stay_open,
                should_exit: false,
                status_message: None,
                entrance: Transition::new(140),
                mode_transition: Transition::new(120),
            },
            load_font_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-launcher".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: iced::Color::WHITE,
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen_with(|event, _status, _window_id| {
            if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) = event
            {
                return Some(Message::CloseRequested);
            }
            Some(Message::IcedEvent(event))
        });
        let animation = if !self.entrance.is_complete() || !self.mode_transition.is_complete() {
            iced::time::every(Duration::from_millis(16)).map(|_| Message::AnimationTick)
        } else {
            Subscription::none()
        };

        if let Some(rx_arc) = &self.ipc_rx {
            let rx_clone = Arc::clone(rx_arc);
            let stream = futures::stream::unfold(rx_clone, |rx| async move {
                let mut lock = rx.lock().await;
                let cmd = lock.recv().await?;
                drop(lock);
                Some((Message::IpcCommand(cmd), rx))
            });
            let ipc_sub = Subscription::run_with_id("corten-launcher-ipc", stream);
            Subscription::batch(vec![events, animation, ipc_sub])
        } else {
            Subscription::batch(vec![events, animation])
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::AnimationTick => {
                self.entrance.tick(16);
                self.mode_transition.tick(16);
            }
            Message::SearchInputChanged(query) => {
                if query.starts_with(":clip") && self.mode != LauncherMode::Clipboard {
                    self.mode = LauncherMode::Clipboard;
                    self.search_query = query.trim_start_matches(":clip").trim().to_string();
                    self.load_clipboard();
                } else if query.starts_with(":p") && self.mode != LauncherMode::Projects {
                    self.mode = LauncherMode::Projects;
                    self.search_query = query.trim_start_matches(":p").trim().to_string();
                    self.filter_projects();
                } else if query.starts_with(":app") && self.mode != LauncherMode::Applications {
                    self.mode = LauncherMode::Applications;
                    self.search_query = query.trim_start_matches(":app").trim().to_string();
                    self.filter_entries();
                } else if (query.starts_with(":emoji") || query.starts_with(":e ")) && self.mode != LauncherMode::Emoji {
                    self.mode = LauncherMode::Emoji;
                    self.search_query = query.trim_start_matches(":emoji").trim_start_matches(":e ").trim().to_string();
                    self.filter_emojis();
                } else if (query.starts_with(":notes") || query.starts_with(":note") || query.starts_with(":n ")) && self.mode != LauncherMode::Notes {
                    self.mode = LauncherMode::Notes;
                    self.search_query = query.trim_start_matches(":notes").trim_start_matches(":note").trim_start_matches(":n ").trim().to_string();
                    self.load_notes_entries();
                } else if (query.starts_with(":keys") || query.starts_with(":cheat") || query.starts_with(":help") || query.starts_with(":k ")) && self.mode != LauncherMode::CheatSheet {
                    self.mode = LauncherMode::CheatSheet;
                    self.search_query = query.trim_start_matches(":keys").trim_start_matches(":cheat").trim_start_matches(":help").trim_start_matches(":k ").trim().to_string();
                    self.selected_index = 0;
                } else {
                    self.search_query = query;
                    self.selected_index = 0;
                    self.filter_current();
                }
            }
            Message::SwitchMode(new_mode) => {
                self.mode_transition.restart();
                self.mode = new_mode;
                self.selected_index = 0;
                self.search_query.clear();
                self.calc_result = None;
                self.filtered_files.clear();
                match self.mode {
                    LauncherMode::Applications => self.filter_entries(),
                    LauncherMode::Projects => self.filter_projects(),
                    LauncherMode::Clipboard => self.load_clipboard(),
                    LauncherMode::Emoji => self.filter_emojis(),
                    LauncherMode::Notes => self.load_notes_entries(),
                    LauncherMode::CheatSheet | LauncherMode::DesktopMenu => {}
                }
            }
            Message::ToggleMode => {
                let next_mode = match self.mode {
                    LauncherMode::Applications => LauncherMode::Projects,
                    LauncherMode::Projects => LauncherMode::Clipboard,
                    LauncherMode::Clipboard => LauncherMode::Emoji,
                    LauncherMode::Emoji => LauncherMode::Notes,
                    LauncherMode::Notes => LauncherMode::CheatSheet,
                    LauncherMode::CheatSheet => LauncherMode::Applications,
                    LauncherMode::DesktopMenu => LauncherMode::DesktopMenu,
                };
                return Task::done(Message::SwitchMode(next_mode));
            }
            Message::SelectDesktopAction(action) => {
                return self.execute_desktop_action(action);
            }
            Message::SelectCategory(cat) => {
                self.selected_category = cat;
                self.selected_index = 0;
                self.filter_entries();
            }
            Message::SelectEmoji(emoji) => {
                let _ = Command::new("wl-copy").arg(&emoji).spawn();
                info!("Emoji copied to clipboard: {}", emoji);
                self.status_message = Some(format!("Kopyalandı: {}", emoji));
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::LaunchSpecificApp(entry) => {
                let launched = if let Err(e) = entry.launch() {
                    error!("Failed to launch app: {}", e);
                    self.status_message = Some(format!("Hata: {}", e));
                    false
                } else {
                    self.status_message = Some(format!("Başlatıldı: {}", entry.name));
                    true
                };
                if launched && !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::CopyCalcResult(val) => {
                let _ = Command::new("wl-copy").arg(&val).spawn();
                self.status_message = Some(format!("Hesaplama sonucu panoya kopyalandı: {}", val));
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::LaunchSpecificFile(entry) => {
                let opened = match Command::new("xdg-open").arg(&entry.path).spawn() {
                    Ok(_) => {
                        self.status_message = Some(format!("Dosya açıldı: {}", entry.name));
                        true
                    }
                    Err(e) => {
                        error!("Failed to open file {}: {}", entry.path.display(), e);
                        self.status_message = Some(format!("Hata: {}", e));
                        false
                    }
                };
                if opened && !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::LaunchSpecificProject(project) => {
                let opened = if let Err(e) = project.open_in_editor() {
                    error!("Failed to open project: {}", e);
                    self.status_message = Some(format!("Hata: {}", e));
                    false
                } else {
                    self.status_message = Some(format!("Proje açıldı: {}", project.name));
                    true
                };

                if opened && !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::OpenProjectTerminal(path) => {
                for terminal in [
                    "corten-terminal",
                    "/usr/local/bin/corten-terminal",
                    "target/release/corten-terminal",
                    "target/debug/corten-terminal",
                    "foot",
                    "alacritty",
                    "kitty",
                    "konsole",
                    "xterm",
                    "pterm",
                ] {
                    if Command::new(terminal).current_dir(&path).spawn().is_ok() {
                        break;
                    }
                }
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::OpenProjectFiles(path) => {
                let path_str = path.to_string_lossy();
                let bin = resolve_binary("corten-files")
                    .unwrap_or_else(|| std::path::PathBuf::from("corten-files"));
                if Command::new(&bin).arg(&*path_str).spawn().is_err() {
                    let _ = Command::new("xdg-open").arg(&*path_str).spawn();
                }
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::CopySpecificClipboard(entry) => {
                let copied = if let Err(e) = ClipboardManager::copy_to_clipboard(&entry) {
                    error!("Failed to copy to clipboard: {}", e);
                    self.status_message = Some(format!("Hata: {}", e));
                    false
                } else {
                    self.status_message = Some("Panoya kopyalandı!".to_string());
                    true
                };

                if copied && !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::SelectPrevious => {
                let total = self.current_count();
                if total > 0 {
                    if self.selected_index == 0 {
                        self.selected_index = total - 1;
                    } else {
                        self.selected_index -= 1;
                    }
                }
            }
            Message::SelectNext => {
                let total = self.current_count();
                if total > 0 {
                    if self.selected_index + 1 >= total {
                        self.selected_index = 0;
                    } else {
                        self.selected_index += 1;
                    }
                }
            }
            Message::ExecuteSelected => match self.mode {
                LauncherMode::Applications => {
                    if self.search_query.starts_with('>') {
                        if let Some(cmd) = self.filtered_commands.get(self.selected_index) {
                            let _ = launch_fallback_chain(cmd.command);
                            if !self.stay_open {
                                self.should_exit = true;
                            }
                        }
                        return Task::none();
                    }

                    if self.search_query.starts_with('?') || self.search_query.starts_with("ai:") {
                        let prompt = self.search_query.trim_start_matches('?').trim_start_matches("ai:").trim();
                        let _ = Command::new("corten-ai").args(["--prompt", prompt]).spawn();
                        if !self.stay_open {
                            self.should_exit = true;
                        }
                        return Task::none();
                    }

                    if self.filtered_entries.is_empty()
                        && self.calc_result.is_none()
                        && self.filtered_files.is_empty()
                        && !self.search_query.trim().is_empty()
                    {
                        let q = self.search_query.trim();
                        let term_cmd = format!("corten-terminal -e {} || alacritty -e {} || foot {} || kitty {} || xterm -e {}", q, q, q, q, q);
                        let _ = launch_fallback_chain(&term_cmd);
                        if !self.stay_open {
                            self.should_exit = true;
                        }
                        return Task::none();
                    }

                    let app_offset = if self.calc_result.is_some() { 1 } else { 0 };
                    if let Some((_, ref res)) = self.calc_result {
                        if self.selected_index == 0 {
                            return Task::done(Message::CopyCalcResult(res.clone()));
                        }
                    }

                    let app_idx = self.selected_index.saturating_sub(app_offset);
                    if let Some(entry) = self.filtered_entries.get(app_idx) {
                        let launched = if let Err(e) = entry.launch() {
                            error!("Failed to launch app: {}", e);
                            self.status_message = Some(format!("Hata: {}", e));
                            false
                        } else {
                            self.status_message = Some(format!("Başlatıldı: {}", entry.name));
                            true
                        };

                        if launched && !self.stay_open {
                            self.should_exit = true;
                        }
                    } else {
                        let file_idx = app_idx.saturating_sub(self.filtered_entries.len());
                        if let Some(file_entry) = self.filtered_files.get(file_idx).cloned() {
                            return Task::done(Message::LaunchSpecificFile(file_entry));
                        }
                    }
                }
                LauncherMode::Projects => {
                    if let Some(proj) = self.filtered_projects.get(self.selected_index) {
                        let opened = if let Err(e) = proj.open_in_editor() {
                            error!("Failed to open project: {}", e);
                            self.status_message = Some(format!("Hata: {}", e));
                            false
                        } else {
                            self.status_message = Some(format!("Proje açıldı: {}", proj.name));
                            true
                        };

                        if opened && !self.stay_open {
                            self.should_exit = true;
                        }
                    }
                }
                LauncherMode::Clipboard => {
                    if let Some(entry) = self.filtered_clipboard.get(self.selected_index) {
                        let copied = if let Err(e) = ClipboardManager::copy_to_clipboard(entry) {
                            error!("Failed to copy to clipboard: {}", e);
                            self.status_message = Some(format!("Hata: {}", e));
                            false
                        } else {
                            self.status_message = Some("Panoya kopyalandı!".to_string());
                            true
                        };

                        if copied && !self.stay_open {
                            self.should_exit = true;
                        }
                    }
                }
                LauncherMode::Emoji => {
                    if let Some(entry) = self.filtered_emojis.get(self.selected_index) {
                        let emoji = entry.emoji.to_string();
                        return Task::done(Message::SelectEmoji(emoji));
                    }
                }
                LauncherMode::Notes => {
                    let trimmed = self.search_query.trim();
                    let has_new_item = !trimmed.is_empty();
                    if has_new_item && self.selected_index == 0 {
                        append_note_file(trimmed);
                        self.search_query.clear();
                        self.load_notes_entries();
                        self.selected_index = 0;
                        self.status_message = Some("📝 Not kaydedildi!".into());
                        return Task::none();
                    }

                    let note_idx = if has_new_item {
                        self.selected_index.saturating_sub(1)
                    } else {
                        self.selected_index
                    };

                    if let Some(note) = self.filtered_notes.get(note_idx) {
                        let clean = note.trim_start_matches("- ").to_string();
                        let mut child = Command::new("wl-copy")
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                            .ok();
                        if let Some(ref mut c) = child {
                            if let Some(ref mut stdin) = c.stdin {
                                use std::io::Write;
                                let _ = stdin.write_all(clean.as_bytes());
                            }
                        }
                        self.status_message = Some("📋 Not panoya kopyalandı!".into());
                        if !self.stay_open {
                            self.should_exit = true;
                        }
                    }
                }
                LauncherMode::CheatSheet => {
                    if !self.stay_open {
                        self.should_exit = true;
                    }
                }
                LauncherMode::DesktopMenu => {
                    let items = self.desktop_menu_items();
                    if let Some(item) = items.get(self.selected_index) {
                        let action = item.action;
                        return self.execute_desktop_action(action);
                    }
                }
            },
            Message::ToggleLauncherStyle => {
                let next_style = match self.config.style {
                    LauncherStyle::Spotlight => LauncherStyle::Dashboard,
                    LauncherStyle::Dashboard => LauncherStyle::Spotlight,
                    LauncherStyle::StartMenu => LauncherStyle::Spotlight,
                };
                self.config.style = next_style;
                if next_style == LauncherStyle::Dashboard && self.search_query.trim().is_empty() {
                    self.filter_entries();
                }
                return Task::none();
            }
            Message::SwitchLauncherStyle(style) => {
                self.config.style = style;
                if style == LauncherStyle::Dashboard && self.search_query.trim().is_empty() {
                    self.filter_entries();
                }
                return Task::none();
            }
            Message::OpenSettings => {
                let bin = resolve_binary("corten-settings")
                    .unwrap_or_else(|| std::path::PathBuf::from("corten-settings"));
                if std::process::Command::new(&bin).spawn().is_err() {
                    let _ = std::process::Command::new("gnome-control-center").spawn();
                }
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::LockScreen => {
                let _ = std::process::Command::new("loginctl")
                    .arg("lock-session")
                    .spawn();
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::PowerOff => {
                let _ = std::process::Command::new("systemctl")
                    .arg("poweroff")
                    .spawn();
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::CloseRequested => {
                self.should_exit = true;
            }
            Message::NoOp => {}
            Message::LaunchCommand(cmd) => {
                match launch_fallback_chain(&cmd) {
                    Ok(_) => {
                        if !self.stay_open {
                            self.should_exit = true;
                        }
                    }
                    Err(e) => {
                        error!("Failed to launch command '{}': {}", cmd, e);
                        self.status_message = Some(format!("Başlatma başarısız: {}", e));
                    }
                }
            }
            Message::IpcCommand(cmd) => match cmd {
                LauncherIpcCommand::Toggle => {
                    self.is_visible = !self.is_visible;
                    if !self.is_visible {
                        self.should_exit = true;
                    }
                }
                LauncherIpcCommand::Open => {
                    self.is_visible = true;
                }
                LauncherIpcCommand::Close => {
                    self.should_exit = true;
                }
                LauncherIpcCommand::ReloadConfig => {
                    info!("Launcher: ReloadConfig received, reading from ~/.config/corten/launcher.toml");
                    if let Ok(new_cfg) =
                        ConfigManager::<LauncherConfig>::load_or_default("launcher")
                    {
                        self.config = new_cfg.get().clone();
                        info!("Launcher config reloaded: {:?}", self.config);
                    }
                }
            },
            Message::IcedEvent(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key,
                modified_key: _,
                physical_key: _,
                location: _,
                modifiers,
                text: _,
            })) => match key {
                Key::Named(Named::Escape) => {
                    self.should_exit = true;
                }
                Key::Named(Named::Tab) => {
                    let next_mode = match self.mode {
                        LauncherMode::Applications => LauncherMode::Projects,
                        LauncherMode::Projects => LauncherMode::Clipboard,
                        LauncherMode::Clipboard => LauncherMode::Emoji,
                        LauncherMode::Emoji => LauncherMode::Notes,
                        LauncherMode::Notes => LauncherMode::CheatSheet,
                        LauncherMode::CheatSheet => LauncherMode::Applications,
                        LauncherMode::DesktopMenu => LauncherMode::DesktopMenu,
                    };
                    self.mode = next_mode;
                    self.selected_index = 0;
                    self.search_query.clear();
                    match self.mode {
                        LauncherMode::Applications => self.filter_entries(),
                        LauncherMode::Projects => self.filter_projects(),
                        LauncherMode::Clipboard => self.load_clipboard(),
                        LauncherMode::Emoji => self.filter_emojis(),
                        LauncherMode::Notes => self.load_notes_entries(),
                        LauncherMode::CheatSheet | LauncherMode::DesktopMenu => {}
                    }
                    return scrollable::snap_to(
                        scrollable::Id::new(SCROLLABLE_ID),
                        scrollable::RelativeOffset::START,
                    );
                }
                ref k if matches!(k, Key::Named(Named::ArrowDown))
                    || matches!(k, Key::Character(ref c) if (c == "j" || c == "J" || c == "n" || c == "N") && modifiers.control()) =>
                {
                    let total = self.current_count();
                    if total > 0 {
                        if self.selected_index + 1 >= total {
                            self.selected_index = 0;
                        } else {
                            self.selected_index += 1;
                        }

                        let offset =
                            self.selected_index as f32 / total.saturating_sub(1).max(1) as f32;
                        return scrollable::snap_to(
                            scrollable::Id::new(SCROLLABLE_ID),
                            scrollable::RelativeOffset { x: 0.0, y: offset },
                        );
                    }
                }
                ref k if matches!(k, Key::Named(Named::ArrowUp))
                    || matches!(k, Key::Character(ref c) if (c == "k" || c == "K" || c == "p" || c == "P") && modifiers.control()) =>
                {
                    let total = self.current_count();
                    if total > 0 {
                        if self.selected_index == 0 {
                            self.selected_index = total - 1;
                        } else {
                            self.selected_index -= 1;
                        }

                        let offset =
                            self.selected_index as f32 / total.saturating_sub(1).max(1) as f32;
                        return scrollable::snap_to(
                            scrollable::Id::new(SCROLLABLE_ID),
                            scrollable::RelativeOffset { x: 0.0, y: offset },
                        );
                    }
                }
                Key::Named(Named::Enter) => {
                    if self.mode == LauncherMode::Applications
                        && self.filtered_entries.is_empty()
                        && self.calc_result.is_none()
                        && self.filtered_files.is_empty()
                        && !self.search_query.trim().is_empty()
                        && !self.search_query.starts_with('>')
                    {
                        let q = self.search_query.trim();
                        if modifiers.control() {
                            let web_cmd = format!("xdg-open 'https://duckduckgo.com/?q={}'", q);
                            let _ = launch_fallback_chain(&web_cmd);
                            if !self.stay_open {
                                self.should_exit = true;
                            }
                            return Task::none();
                        } else if modifiers.alt() {
                            let files_cmd = format!("corten-files --search '{}'", q);
                            let _ = launch_fallback_chain(&files_cmd);
                            if !self.stay_open {
                                self.should_exit = true;
                            }
                            return Task::none();
                        }
                    }
                    return self.update(Message::ExecuteSelected);
                }
                _ => {}
            },
            Message::IcedEvent(Event::Window(iced::window::Event::Unfocused)) => {
                if !self.stay_open {
                    self.should_exit = true;
                }
            }
            Message::IcedEvent(_) | Message::LayerAction(_) => {}
        }
        if self.should_exit {
            std::process::exit(0);
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        let entrance_alpha = (0.72 + 0.28 * self.entrance.eased()).clamp(0.0, 1.0);
        let content_alpha = (0.82 + 0.18 * self.mode_transition.eased()).clamp(0.0, 1.0);

        if self.mode == LauncherMode::DesktopMenu {
            return self.view_desktop_context_menu(palette, entrance_alpha, content_alpha);
        }

        if self.config.style == LauncherStyle::StartMenu {
            // ==================== CORTEN START MENU (PIXEL PERFECT & VECTOR ICONS) ====================
            let close_btn = button(
                text(Icon::X)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([4, 8])
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.25),
                    _ => iced::Color::TRANSPARENT,
                };
                button::Style {
                    background: Some(bg.into()),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::CloseRequested);

            let search_box = container(
                row![
                    text(Icon::SEARCH)
                        .font(LUCIDE_FONT)
                        .size(15.0)
                        .color(palette.primary.to_iced()),
                    text_input("Uygulama ve dosya ara...", &self.search_query)
                        .size(13.0)
                        .on_input(Message::SearchInputChanged)
                        .on_submit(Message::ExecuteSelected)
                        .width(Length::Fill)
                        .padding([4, 6])
                        .style(move |_, _| text_input::Style {
                            background: iced::Background::Color(iced::Color::TRANSPARENT),
                            border: iced::Border::default(),
                            icon: palette.primary.to_iced(),
                            placeholder: palette.text_muted.to_iced(),
                            value: palette.text_primary.to_iced(),
                            selection: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.4,
                            ),
                        }),
                    close_btn,
                ]
                .spacing(8)
                .padding([6, 10])
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.06,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.40,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(10.0),
                },
                ..Default::default()
            });

            // Quick Access Favorites Row
            let fav_tile = |label: &'static str, icon_char: char, cmd: &'static str| {
                button(
                    iced::widget::column![
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(18.0)
                            .color(palette.primary.to_iced()),
                        text(label).size(11.0).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(5)
                    .align_x(Alignment::Center),
                )
                .width(Length::FillPortion(1))
                .height(Length::Fixed(56.0))
                .padding([6, 4])
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: iced::border::Radius::from(10.0),
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::LaunchCommand(cmd.into()))
            };

            let favorites_row = row![
                fav_tile("Terminal", Icon::TERMINAL, "corten-terminal || foot || alacritty || kitty || pterm || konsole || xterm"),
                fav_tile("Dosyalar", Icon::FOLDER, "corten-files || nautilus || thunar || pcmanfm || dolphin || ark || xdg-open ~"),
                fav_tile("Tarayıcı", Icon::GLOBE, "brave || zen-browser || firefox || chromium || google-chrome-stable || xdg-open https://"),
                fav_tile("Kod", Icon::CODE, "code || zeditor || antigravity-ide || codium || gedit || kate || vim"),
            ]
            .spacing(8)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            // Categories pill filters
            let cat_pill = |label: &'static str, cat: AppCategory, is_active: bool| {
                button(text(label).size(11.0).color(if is_active {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }))
                .padding([4, 10])
                .style(move |_, status| {
                    let bg = if is_active {
                        iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.18)
                    } else {
                        match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                            _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                        }
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: if is_active {
                                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4)
                            } else {
                                iced::Color::TRANSPARENT
                            },
                            width: 1.0,
                            radius: iced::border::Radius::from(10.0),
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::SelectCategory(cat))
            };

            let categories_row = row![
                cat_pill("Tümü", AppCategory::All, self.selected_category == AppCategory::All),
                cat_pill("Geliştirme", AppCategory::Development, self.selected_category == AppCategory::Development),
                cat_pill("Grafik", AppCategory::Media, self.selected_category == AppCategory::Media),
                cat_pill("İnternet", AppCategory::Internet, self.selected_category == AppCategory::Internet),
                cat_pill("Ofis", AppCategory::Office, self.selected_category == AppCategory::Office),
                cat_pill("Sistem", AppCategory::System, self.selected_category == AppCategory::System),
            ]
            .spacing(5)
            .align_y(Alignment::Center);

            // Applications List Section Header
            let section_title = if !self.search_query.trim().is_empty() {
                format!("Arama Sonuçları: \"{}\"", self.search_query)
            } else {
                match self.selected_category {
                    AppCategory::All => "Tüm Uygulamalar".to_string(),
                    AppCategory::Development => "Geliştirme Uygulamaları".to_string(),
                    AppCategory::Media => "Grafik ve Medya Uygulamaları".to_string(),
                    AppCategory::Internet => "İnternet Uygulamaları".to_string(),
                    AppCategory::Office => "Ofis Uygulamaları".to_string(),
                    AppCategory::System => "Sistem Uygulamaları".to_string(),
                }
            };

            // Applications Scrollable List
            let mut apps_list = iced::widget::column![].spacing(4).width(Length::Fill);

            if let Some((ref expr, ref calc_res)) = self.calc_result {
                let res_clone = calc_res.clone();
                let calc_card = button(
                    row![
                        text("🧮").size(18.0),
                        iced::widget::column![
                            text(format!("= {}", calc_res))
                                .size(15.0)
                                .font(iced::Font {
                                    weight: iced::font::Weight::Bold,
                                    ..Default::default()
                                })
                                .color(palette.primary.to_iced()),
                            text(format!("Hesaplama: {} (Kopyalamak için tıklayın)", expr))
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([8, 10])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.16,
                    ))),
                    border: iced::Border {
                        color: palette.primary.to_iced(),
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                })
                .on_press(Message::CopyCalcResult(res_clone));
                apps_list = apps_list.push(calc_card);
            }

            if self.search_query.starts_with('>') {
                apps_list = apps_list.push(render_command_list(&self.filtered_commands, self.selected_index, palette));
            } else if self.filtered_entries.is_empty() && self.calc_result.is_none() {
                apps_list = apps_list.push(render_smart_empty_state(&self.search_query, palette));
            } else {
                for (idx, entry) in self.filtered_entries.iter().take(120).enumerate() {
                    let is_sel = idx == self.selected_index;
                    let entry_clone = entry.clone();
                    let icon_view = render_app_icon(entry, 28.0, palette.primary.to_iced());

                    let active_strip = container(text(""))
                        .width(Length::Fixed(3.0))
                        .height(Length::Fixed(20.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(if is_sel {
                                palette.primary.to_iced()
                            } else {
                                iced::Color::TRANSPARENT
                            })),
                            border: iced::Border {
                                radius: iced::border::Radius::from(2.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                    let app_icon_box = container(icon_view)
                        .width(Length::Fixed(32.0))
                        .height(Length::Fixed(32.0))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill);

                    let item_btn = button(
                        row![
                            active_strip,
                            app_icon_box,
                            iced::widget::column![
                                text(&entry.name)
                                    .size(13.0)
                                    .font(iced::Font {
                                        weight: iced::font::Weight::Semibold,
                                        ..Default::default()
                                    })
                                    .color(if is_sel {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_primary.to_iced()
                                    }),
                                text(entry.comment.as_deref().unwrap_or(&entry.exec))
                                    .size(11.0)
                                    .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(2)
                            .width(Length::Fill),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([5, 8])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                            _ if is_sel => iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.18,
                            ),
                            _ => iced::Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                color: if is_sel {
                                    palette.primary.to_iced()
                                } else {
                                    iced::Color::TRANSPARENT
                                },
                                width: 1.0,
                                radius: iced::border::Radius::from(8.0),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::LaunchSpecificApp(entry_clone));

                    apps_list = apps_list.push(item_btn);
                }
            }

            // Bottom Footer with Settings and Power Off
            let footer_row = row![
                button(
                    row![
                        text(Icon::SETTINGS)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(palette.text_muted.to_iced()),
                        text("Ayarlar").size(12.0).color(palette.text_muted.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .on_press(Message::OpenSettings)
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        _ => iced::Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
                button(
                    row![
                        text(Icon::APP_WINDOW)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(palette.primary.to_iced()),
                        text("Izgara").size(12.0).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .on_press(Message::SwitchLauncherStyle(LauncherStyle::Dashboard))
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        _ => iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.10),
                    };
                    button::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.25),
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..Default::default()
                    }
                }),
                button(
                    row![
                        text(Icon::SEARCH)
                            .font(LUCIDE_FONT)
                            .size(13.0)
                            .color(palette.primary.to_iced()),
                        text("Spotlight").size(12.0).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .on_press(Message::SwitchLauncherStyle(LauncherStyle::Spotlight))
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    };
                    button::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..Default::default()
                    }
                }),
                horizontal_space(),
                button(
                    row![
                        text(Icon::POWER)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(palette.danger.to_iced()),
                        text("Kapat").size(12.0).color(palette.danger.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .on_press(Message::PowerOff)
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.2),
                        _ => iced::Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(bg.into()),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            ]
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .padding([4, 4]);

            let menu_card = container(
                iced::widget::column![
                    search_box,
                    favorites_row,
                    categories_row,
                    text(section_title)
                        .size(12.0)
                        .font(iced::Font {
                            weight: iced::font::Weight::Semibold,
                            ..Default::default()
                        })
                        .color(palette.primary.to_iced()),
                    scrollable(apps_list)
                        .height(Length::Fixed(320.0))
                        .width(Length::Fill),
                    footer_row,
                ]
                .spacing(10)
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .padding([14, 16])
            .width(Length::Fixed(520.0))
            .height(Length::Fixed(590.0))
            .style(move |_| container::Style {
                // Frosted acrylic dark glass with sleek Corten rust border glow
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.12, 0.95,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.45,
                    ),
                    width: 1.5,
                    radius: iced::border::Radius::from(16.0),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                    offset: iced::Vector::new(0.0, 10.0),
                    blur_radius: 28.0,
                },
                ..Default::default()
            });

            let inner_card = mouse_area(menu_card).on_press(Message::NoOp);

            return mouse_area(
                container(inner_card)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Bottom)
                    .padding(iced::Padding {
                        top: 0.0,
                        right: 0.0,
                        bottom: 84.0,
                        left: 0.0,
                    }),
            )
            .on_press(Message::CloseRequested)
            .into();
        }

        // Header Mode Tabs
        let mode_tabs = row![
            button(text("Uygulamalar").size(Typography::CAPTION).color(
                if self.mode == LauncherMode::Applications {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }
            ))
            .padding([4, 12])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(
                    if self.mode == LauncherMode::Applications {
                        palette.surface_hover.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    }
                )),
                text_color: palette.text_primary.to_iced(),
                border: iced::Border {
                    color: if self.mode == LauncherMode::Applications {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(Message::SwitchMode(LauncherMode::Applications)),
            button(
                text("Projeler (Geliştirme)")
                    .size(Typography::CAPTION)
                    .color(if self.mode == LauncherMode::Projects {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    })
            )
            .padding([4, 12])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(
                    if self.mode == LauncherMode::Projects {
                        palette.surface_hover.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    }
                )),
                text_color: palette.text_primary.to_iced(),
                border: iced::Border {
                    color: if self.mode == LauncherMode::Projects {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(Message::SwitchMode(LauncherMode::Projects)),
            button(text("Pano (cliphist)").size(Typography::CAPTION).color(
                if self.mode == LauncherMode::Clipboard {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }
            ))
            .padding([4, 12])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(
                    if self.mode == LauncherMode::Clipboard {
                        palette.surface_hover.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    }
                )),
                text_color: palette.text_primary.to_iced(),
                border: iced::Border {
                    color: if self.mode == LauncherMode::Clipboard {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(Message::SwitchMode(LauncherMode::Clipboard)),
            button(text("Emoji (:e)").size(Typography::CAPTION).color(
                if self.mode == LauncherMode::Emoji {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }
            ))
            .padding([4, 12])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(
                    if self.mode == LauncherMode::Emoji {
                        palette.surface_hover.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    }
                )),
                text_color: palette.text_primary.to_iced(),
                border: iced::Border {
                    color: if self.mode == LauncherMode::Emoji {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(Message::SwitchMode(LauncherMode::Emoji)),
            button(
                row![
                    text("📝").size(14.0),
                    text("Notlar").size(Typography::BODY_SM)
                ]
                .spacing(Spacing::XXS)
                .align_y(Alignment::Center),
            )
            .padding([4, 10])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(
                    if self.mode == LauncherMode::Notes {
                        palette.surface_hover.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    }
                )),
                text_color: palette.text_primary.to_iced(),
                border: iced::Border {
                    color: if self.mode == LauncherMode::Notes {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
            })
            .on_press(Message::SwitchMode(LauncherMode::Notes)),
            Space::new(Length::Fill, Length::Shrink),
            button(
                text("☰ Başlat")
                    .size(Typography::CAPTION)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([4, 10])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.18),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    shadow: iced::Shadow::default(),
                    ..Default::default()
                }
            })
            .on_press(Message::SwitchLauncherStyle(LauncherStyle::StartMenu)),
            button(
                text(match self.config.style {
                    LauncherStyle::Spotlight => "⊞ Izgara Görünümü",
                    LauncherStyle::Dashboard => "🔍 Spotlight Görünümü",
                    LauncherStyle::StartMenu => "⊞ Izgara Görünümü",
                })
                .size(Typography::CAPTION)
                .color(palette.primary.to_iced())
            )
            .padding([4, 10])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.85, 0.42, 0.26, 0.15
                ))),
                border: iced::Border {
                    color: palette.primary.to_iced(),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
                ..Default::default()
            })
            .on_press(Message::ToggleLauncherStyle),
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center)
        .width(Length::Fill);

        // Search bar with Lucide icon
        let t = self.language.t();
        let placeholder = match self.mode {
            LauncherMode::Applications => &self.config.placeholder,
            LauncherMode::Projects => "Search git repositories & dev projects (:p)...",
            LauncherMode::Clipboard => "Pano geçmişinde ara (:clip)...",
            LauncherMode::Emoji => "Emoji veya özel simge ara (:e)...",
            LauncherMode::Notes => "Not yaz ve Enter'a bas, veya notlarda ara (:n)...",
            LauncherMode::CheatSheet => t.cheat_sheet_subtitle,
            LauncherMode::DesktopMenu => "Masaüstü eylemleri...",
        };

        let search_icon = match self.mode {
            LauncherMode::Applications => Icon::TERMINAL,
            LauncherMode::Projects => Icon::CODE,
            LauncherMode::Clipboard => Icon::SEARCH,
            LauncherMode::Emoji => Icon::SMILE,
            LauncherMode::Notes => Icon::CLIPBOARD,
            LauncherMode::CheatSheet => Icon::KEYBOARD,
            LauncherMode::DesktopMenu => Icon::APP_WINDOW,
        };

        let search_bar = container(
            row![
                text(search_icon)
                    .font(LUCIDE_FONT)
                    .size(Typography::TITLE_SM)
                    .color(palette.primary.to_iced()),
                text_input(placeholder, &self.search_query)
                    .size(Typography::TITLE_SM)
                    .on_input(Message::SearchInputChanged)
                    .width(Length::Fill),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding(Spacing::MD)
        .style(move |_| input_style(palette));

        // Category pills row when in Applications mode
        let category_pills: Element<'_, Self::Message> = if self.mode == LauncherMode::Applications
        {
            let cat_pill = |cat: AppCategory, label: &'static str| {
                let is_active = self.selected_category == cat;
                button(text(label).size(11.0).color(if is_active {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }))
                .padding([2.0, 8.0])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(if is_active {
                        iced::Color::from_rgba(0.85, 0.42, 0.26, 0.18)
                    } else {
                        iced::Color::TRANSPARENT
                    })),
                    border: iced::Border {
                        color: if is_active {
                            palette.primary.to_iced()
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                        },
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                })
                .on_press(Message::SelectCategory(cat))
            };

            let quick_chip = |icon: &'static str, label: &'static str, msg: Message| {
                button(
                    row![
                        text(icon).size(11.0),
                        text(label).size(10.5).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([2.0, 7.0])
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::FULL),
                        },
                        ..Default::default()
                    }
                })
                .on_press(msg)
            };

            let quick_chips_row = row![
                quick_chip("⚡", "AI Sor (?)", Message::SearchInputChanged("?".into())),
                quick_chip("📋", "Pano", Message::SwitchMode(LauncherMode::Clipboard)),
                quick_chip("📦", "Projeler", Message::SwitchMode(LauncherMode::Projects)),
                quick_chip("😀", "Emojiler", Message::SwitchMode(LauncherMode::Emoji)),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            row![
                cat_pill(AppCategory::All, "Tümü"),
                cat_pill(AppCategory::Internet, "İnternet"),
                cat_pill(AppCategory::Office, "Ofis"),
                cat_pill(AppCategory::Media, "Medya"),
                cat_pill(AppCategory::Development, "Geliştirme"),
                cat_pill(AppCategory::System, "Sistem"),
                Space::new(Length::Fill, Length::Shrink),
                quick_chips_row,
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
        } else {
            Space::new(Length::Shrink, Length::Shrink).into()
        };

        // Search results list
        let mut results_column = column![].spacing(Spacing::XXS).width(Length::Fill);

        match self.mode {
            LauncherMode::Applications => {
                let app_offset = if self.calc_result.is_some() { 1 } else { 0 };

                // 1. Math Evaluator result row if active
                if let Some((expr, calc_res)) = &self.calc_result {
                    let is_selected = self.selected_index == 0;
                    let (bg, border_color) = if is_selected {
                        (palette.surface_hover.to_iced(), palette.primary.to_iced())
                    } else {
                        (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                    };

                    let calc_row = button(
                        row![
                            text("🧮").size(20.0),
                            column![
                                text(format!("= {}", calc_res))
                                    .size(Typography::TITLE_SM)
                                    .color(palette.primary.to_iced()),
                                text(format!("Hesaplama: {}", expr))
                                    .size(Typography::CAPTION)
                                    .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(2.0)
                            .width(Length::Fill),
                            text("Panoya Kopyala ↵")
                                .size(Typography::CAPTION)
                                .color(if is_selected {
                                    palette.primary.to_iced()
                                } else {
                                    palette.text_muted.to_iced()
                                }),
                        ]
                        .spacing(Spacing::SM)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .padding(Spacing::SM)
                    .width(Length::Fill)
                    .style(move |_, _| button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: palette.text_primary.to_iced(),
                        border: iced::Border {
                            color: border_color,
                            width: if is_selected { 1.0 } else { 0.0 },
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        shadow: iced::Shadow::default(),
                    })
                    .on_press(Message::CopyCalcResult(calc_res.clone()));

                    results_column = results_column.push(calc_row);
                }

                // 2. Applications
                if self.search_query.is_empty() && !self.filtered_entries.is_empty() {
                    let mut fav_row = row![
                        text("★ Sık Kullanılanlar:")
                            .size(11.0)
                            .color(palette.primary.to_iced())
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);

                    for fav in self.filtered_entries.iter().take(5) {
                        let fav_clone = fav.clone();
                        let fav_icon: Element<'_, Self::Message> = if let Some(ref ip) = fav.icon_path {
                            iced::widget::image(image::Handle::from_path(ip))
                                .width(Length::Fixed(16.0))
                                .height(Length::Fixed(16.0))
                                .into()
                        } else {
                            text(app_icon_char(fav))
                                .font(LUCIDE_FONT)
                                .size(12.0)
                                .color(palette.primary.to_iced())
                                .into()
                        };
                        let fav_btn = button(
                            row![
                                fav_icon,
                                text(&fav.name)
                                    .size(11.0)
                                    .color(palette.text_primary.to_iced()),
                            ]
                            .spacing(5)
                            .align_y(Alignment::Center),
                        )
                        .padding([2, 8])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .on_press(Message::LaunchSpecificApp(fav_clone));
                        fav_row = fav_row.push(fav_btn);
                    }
                    results_column = results_column.push(container(fav_row).padding([4, 6]));
                }

                if self.search_query.starts_with('>') {
                    results_column = results_column.push(render_command_list(&self.filtered_commands, self.selected_index, palette));
                } else if self.filtered_entries.is_empty()
                    && self.calc_result.is_none()
                    && self.filtered_files.is_empty()
                {
                    results_column = results_column.push(render_smart_empty_state(&self.search_query, palette));
                } else {
                    for (idx, entry) in self.filtered_entries.iter().enumerate() {
                        let is_selected = (idx + app_offset) == self.selected_index;
                        let entry_to_launch = entry.clone();

                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        let active_strip = container(text(""))
                            .width(Length::Fixed(3.0))
                            .height(Length::Fixed(20.0))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_selected {
                                    palette.primary.to_iced()
                                } else {
                                    iced::Color::TRANSPARENT
                                })),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(2.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });

                        let item_icon = render_app_icon(entry, 22.0, palette.primary.to_iced());

                        let row_item = button(
                            row![
                                active_strip,
                                item_icon,
                                column![
                                    text(&entry.name)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced()),
                                    text(entry.generic_name.as_deref().unwrap_or(&entry.exec))
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2.0),
                            ]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                        )
                        .padding([6.0, 10.0])
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: if is_selected { 1.0 } else { 0.0 },
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::LaunchSpecificApp(entry_to_launch));

                        results_column = results_column.push(row_item);
                    }

                    // 3. Files & Folders section
                    if !self.filtered_files.is_empty() {
                        results_column = results_column.push(
                            container(
                                text("📁 Dosyalar ve Klasörler")
                                    .size(Typography::CAPTION)
                                    .color(palette.primary.to_iced()),
                            )
                            .padding([8, 6]),
                        );

                        let file_offset = app_offset + self.filtered_entries.len();
                        for (f_idx, file) in self.filtered_files.iter().enumerate() {
                            let is_selected = (file_offset + f_idx) == self.selected_index;
                            let file_to_open = file.clone();

                            let (bg, border_color) = if is_selected {
                                (palette.surface_hover.to_iced(), palette.primary.to_iced())
                            } else {
                                (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                            };

                            let file_icon = if file.is_dir {
                                Icon::FOLDER
                            } else {
                                Icon::APP_WINDOW
                            };

                            let file_row = button(
                                row![
                                    text(file_icon)
                                        .font(LUCIDE_FONT)
                                        .size(Typography::BODY)
                                        .color(if is_selected {
                                            palette.primary.to_iced()
                                        } else {
                                            palette.text_muted.to_iced()
                                        }),
                                    column![
                                        text(&file.name)
                                            .size(Typography::BODY)
                                            .color(palette.text_primary.to_iced()),
                                        text(file.path.to_string_lossy().to_string())
                                            .size(Typography::CAPTION)
                                            .color(palette.text_muted.to_iced()),
                                    ]
                                    .spacing(2.0)
                                    .width(Length::Fill),
                                    text("Aç ↵")
                                        .size(Typography::CAPTION)
                                        .color(if is_selected {
                                            palette.primary.to_iced()
                                        } else {
                                            palette.text_muted.to_iced()
                                        }),
                                ]
                                .spacing(Spacing::SM)
                                .align_y(Alignment::Center)
                                .width(Length::Fill),
                            )
                            .padding(Spacing::SM)
                            .width(Length::Fill)
                            .style(move |_, _| button::Style {
                                background: Some(iced::Background::Color(bg)),
                                text_color: palette.text_primary.to_iced(),
                                border: iced::Border {
                                    color: border_color,
                                    width: if is_selected { 1.0 } else { 0.0 },
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                shadow: iced::Shadow::default(),
                            })
                            .on_press(Message::LaunchSpecificFile(file_to_open));

                            results_column = results_column.push(file_row);
                        }
                    }
                }
            }
            LauncherMode::Projects => {
                if self.filtered_projects.is_empty() {
                    results_column = results_column.push(
                        container(
                            column![
                                text("Geliştirici projesi bulunamadı")
                                    .size(Typography::BODY)
                                    .color(palette.text_muted.to_iced()),
                                text("~/Projects, ~/code veya ~/src içinde klasör oluşturun")
                                    .size(Typography::CAPTION)
                                    .color(palette.border.to_iced()),
                            ]
                            .spacing(Spacing::XXS)
                            .align_x(Alignment::Center),
                        )
                        .padding(Spacing::LG)
                        .center_x(Length::Fill),
                    );
                } else {
                    for (idx, proj) in self.filtered_projects.iter().enumerate() {
                        let is_selected = idx == self.selected_index;
                        let project_to_launch = proj.clone();

                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        let proj_badge = container(
                            text(proj.project_type)
                                .size(10.0)
                                .color(palette.primary.to_iced()),
                        )
                        .padding([2, 6])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.15,
                            ))),
                            border: iced::Border {
                                color: palette.primary.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        });

                        let row_item = button(
                            row![
                                text(Icon::CODE)
                                    .font(LUCIDE_FONT)
                                    .size(Typography::BODY)
                                    .color(if is_selected {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                                column![
                                    text(&proj.name)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced()),
                                    text(proj.path.to_string_lossy())
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2.0)
                                .width(Length::Fill),
                                proj_badge,
                            ]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                        )
                        .padding(Spacing::SM)
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: if is_selected { 1.0 } else { 0.0 },
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::LaunchSpecificProject(project_to_launch));

                        results_column = results_column.push(row_item);
                    }
                }
            }
            LauncherMode::Clipboard => {
                if !ClipboardManager::is_available() {
                    results_column = results_column.push(
                        container(
                            text("cliphist veya wl-clipboard PATH içinde bulunamadı.\nPano geçmişi için bu paketleri kurun.")
                                .size(Typography::BODY)
                                .color(palette.warning.to_iced())
                        )
                        .padding(Spacing::MD)
                    );
                } else if self.filtered_clipboard.is_empty() {
                    results_column = results_column.push(
                        container(
                            text("Pano geçmişi boş")
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding(Spacing::MD),
                    );
                } else {
                    for (idx, entry) in self.filtered_clipboard.iter().enumerate() {
                        let is_selected = idx == self.selected_index;
                        let entry_to_copy = entry.clone();

                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        let preview_trim = entry.preview.trim();
                        let type_badge: Element<'_, Self::Message> = if preview_trim
                            .starts_with("http://")
                            || preview_trim.starts_with("https://")
                        {
                            container(text("URL").size(10.0).color(palette.primary.to_iced()))
                                .padding([2, 6])
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        iced::Color::from_rgba(
                                            palette.primary.r,
                                            palette.primary.g,
                                            palette.primary.b,
                                            0.15,
                                        ),
                                    )),
                                    border: iced::Border {
                                        color: palette.primary.to_iced(),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::SM),
                                    },
                                    ..Default::default()
                                })
                                .into()
                        } else if preview_trim.starts_with('#')
                            && (preview_trim.len() == 4
                                || preview_trim.len() == 7
                                || preview_trim.len() == 9)
                        {
                            container(text("COLOR").size(10.0).color(palette.warning.to_iced()))
                                .padding([2, 6])
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        iced::Color::from_rgba(
                                            palette.warning.r,
                                            palette.warning.g,
                                            palette.warning.b,
                                            0.15,
                                        ),
                                    )),
                                    border: iced::Border {
                                        color: palette.warning.to_iced(),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::SM),
                                    },
                                    ..Default::default()
                                })
                                .into()
                        } else {
                            container(text("TEXT").size(10.0).color(palette.text_muted.to_iced()))
                                .padding([2, 6])
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        palette.surface_hover.to_iced(),
                                    )),
                                    border: iced::Border {
                                        color: palette.border.to_iced(),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::SM),
                                    },
                                    ..Default::default()
                                })
                                .into()
                        };

                        let row_item = button(
                            row![
                                text(Icon::CLIPBOARD)
                                    .font(LUCIDE_FONT)
                                    .size(Typography::BODY)
                                    .color(if is_selected {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                                text(&entry.preview)
                                    .size(Typography::BODY)
                                    .color(palette.text_primary.to_iced())
                                    .width(Length::Fill),
                                type_badge,
                            ]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                        )
                        .padding(Spacing::SM)
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: if is_selected { 1.0 } else { 0.0 },
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::CopySpecificClipboard(entry_to_copy));

                        results_column = results_column.push(row_item);
                    }
                }
            }
            LauncherMode::Emoji => {
                if self.filtered_emojis.is_empty() {
                    results_column = results_column.push(
                        container(
                            text("Eşleşen emoji bulunamadı")
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding(Spacing::MD),
                    );
                } else {
                    for (idx, item) in self.filtered_emojis.iter().enumerate() {
                        let is_selected = idx == self.selected_index;
                        let emoji_str = item.emoji.to_string();

                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        let row_item = button(
                            row![
                                text(item.emoji).size(26.0),
                                column![
                                    text(item.name)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced()),
                                    text(format!("{} • {}", item.category, item.keywords))
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2)
                                .width(Length::Fill),
                                text("Kopyala ↵")
                                    .size(Typography::CAPTION)
                                    .color(if is_selected {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                            ]
                            .spacing(Spacing::MD)
                            .align_y(Alignment::Center),
                        )
                        .padding([8, 12])
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::SelectEmoji(emoji_str));

                        results_column = results_column.push(row_item);
                    }
                }
            }
            LauncherMode::Notes => {
                let trimmed = self.search_query.trim();
                let has_new_item = !trimmed.is_empty();

                if has_new_item {
                    let is_selected = self.selected_index == 0;
                    let (bg, border_color) = if is_selected {
                        (palette.surface_hover.to_iced(), palette.primary.to_iced())
                    } else {
                        (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                    };
                    let add_item = button(
                        row![
                            text("➕").size(20.0),
                            column![
                                text(format!("Yeni Not Kaydet: \"{}\"", trimmed))
                                    .size(Typography::BODY)
                                    .color(palette.primary.to_iced()),
                                text("Enter'a basarak notu listeye ekleyin")
                                    .size(Typography::CAPTION)
                                    .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(2)
                            .width(Length::Fill),
                            text("Kaydet ↵")
                                .size(Typography::CAPTION)
                                .color(palette.primary.to_iced()),
                        ]
                        .spacing(Spacing::MD)
                        .align_y(Alignment::Center),
                    )
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(move |_, _| button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: palette.text_primary.to_iced(),
                        border: iced::Border {
                            color: border_color,
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        shadow: iced::Shadow::default(),
                    })
                    .on_press(Message::ExecuteSelected);
                    results_column = results_column.push(add_item);
                }

                if self.filtered_notes.is_empty() && !has_new_item {
                    results_column = results_column.push(
                        container(
                            text("Henüz kayıtlı notunuz yok. Bir şeyler yazıp Enter'a basın!")
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding(Spacing::MD),
                    );
                } else {
                    for (idx, note_line) in self.filtered_notes.iter().enumerate() {
                        let actual_idx = if has_new_item { idx + 1 } else { idx };
                        let is_selected = actual_idx == self.selected_index;
                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        let clean_line = note_line.trim_start_matches("- ");

                        let row_item = button(
                            row![
                                text("📝").size(18.0),
                                column![
                                    text(clean_line)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced()),
                                ]
                                .spacing(2)
                                .width(Length::Fill),
                                text("Kopyala ↵")
                                    .size(Typography::CAPTION)
                                    .color(if is_selected {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                            ]
                            .spacing(Spacing::MD)
                            .align_y(Alignment::Center),
                        )
                        .padding([8, 12])
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::ExecuteSelected);

                        results_column = results_column.push(row_item);
                    }
                }
            }
            LauncherMode::CheatSheet => {
                let shortcuts = self.filtered_shortcuts();
                if shortcuts.is_empty() {
                    results_column = results_column.push(
                        container(
                            text("Eşleşen klavye kısayolu bulunamadı")
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding(Spacing::LG)
                        .center_x(Length::Fill),
                    );
                } else {
                    for (idx, sc) in shortcuts.iter().enumerate() {
                        let is_selected = idx == self.selected_index;
                        let (bg, border_color) = if is_selected {
                            (palette.surface_hover.to_iced(), palette.primary.to_iced())
                        } else {
                            (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                        };

                        // Key cap badges
                        let chords_row = row(sc.chords.split('+').map(|part| {
                            container(
                                text(part.trim())
                                    .size(11.0)
                                    .color(palette.primary.to_iced())
                            )
                            .padding([2, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                                border: iced::Border {
                                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.3),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(4.0),
                                },
                                ..Default::default()
                            })
                            .into()
                        }))
                        .spacing(3)
                        .align_y(Alignment::Center);

                        let row_item = button(
                            row![
                                container(
                                    text(sc.icon_char)
                                        .font(LUCIDE_FONT)
                                        .size(16.0)
                                        .color(if is_selected { palette.primary.to_iced() } else { palette.text_muted.to_iced() })
                                )
                                .padding([4, 6]),
                                chords_row,
                                column![
                                    text(sc.title)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced()),
                                    text(sc.desc)
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2)
                                .width(Length::Fill),
                                container(
                                    text(sc.category)
                                        .size(10.0)
                                        .color(palette.text_disabled.to_iced())
                                )
                                .padding([2, 6])
                                .style(|_| container::Style {
                                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(4.0),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                            ]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 10])
                        .width(Length::Fill)
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(bg)),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: border_color,
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::ExecuteSelected);

                        results_column = results_column.push(row_item);
                    }
                }
            }
            LauncherMode::DesktopMenu => {
                let items = self.desktop_menu_items();
                for (i, item) in items.into_iter().enumerate() {
                    let is_selected = self.selected_index == i;
                    let (bg, border_color) = if is_selected {
                        (palette.surface_hover.to_iced(), palette.primary.to_iced())
                    } else {
                        (palette.surface.to_iced(), iced::Color::TRANSPARENT)
                    };

                    let keycap = container(
                        text(item.shortcut)
                            .size(11.0)
                            .font(iced::Font::MONOSPACE)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding([2, 6])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.background.to_iced())),
                        border: iced::Border {
                            color: palette.border.to_iced(),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        ..Default::default()
                    });

                    let row_item = button(
                        row![
                            text(item.icon_char)
                                .font(LUCIDE_FONT)
                                .size(16.0)
                                .color(if is_selected {
                                    palette.primary.to_iced()
                                } else {
                                    palette.text_muted.to_iced()
                                }),
                            Space::new(Length::Fixed(6.0), Length::Shrink),
                            text(item.title)
                                .size(Typography::BODY)
                                .color(palette.text_primary.to_iced())
                                .width(Length::Fill),
                            keycap,
                        ]
                        .spacing(Spacing::XS)
                        .align_y(Alignment::Center)
                        .padding([8, 12]),
                    )
                    .width(Length::Fill)
                    .style(move |_, _| button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: palette.text_primary.to_iced(),
                        border: iced::Border {
                            color: border_color,
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        shadow: iced::Shadow::default(),
                    })
                    .on_press(Message::SelectDesktopAction(item.action));

                    results_column = results_column.push(row_item);
                }
            }
        }

        // Status or helper footer with user profile and quick actions
        let username = std::env::var("USER").unwrap_or_else(|_| "corten".to_string());
        let user_pill = row![
            text("👤")
                .size(Typography::CAPTION)
                .color(palette.primary.to_iced()),
            text(username)
                .size(Typography::CAPTION)
                .color(palette.text_primary.to_iced()),
        ]
        .spacing(Spacing::XXS)
        .align_y(Alignment::Center);

        let quick_btn = |label: &'static str, msg: Message| {
            button(
                text(label)
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([2.0, 6.0])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.surface_hover.to_iced())),
                border: iced::Border {
                    color: palette.border.to_iced(),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                shadow: iced::Shadow::default(),
                ..Default::default()
            })
            .on_press(msg)
        };

        let quick_actions = row![
            quick_btn("⚙ Ayarlar", Message::OpenSettings),
            quick_btn("🔒 Kilitle", Message::LockScreen),
            quick_btn("⏻ Güç", Message::PowerOff),
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center);

        let key_badge = |key: &'static str, desc: &'static str| {
            row![
                container(
                    text(key)
                        .size(10.0)
                        .color(palette.primary.to_iced()),
                )
                .padding([2, 5])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4),
                        width: 1.0,
                        radius: iced::border::Radius::from(4.0),
                    },
                    ..Default::default()
                }),
                text(desc)
                    .size(10.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
        };

        let footer_shortcuts = if let Some(msg) = &self.status_message {
            row![text(msg)
                .size(Typography::CAPTION)
                .color(palette.primary.to_iced())]
            .align_y(Alignment::Center)
        } else {
            row![
                key_badge("Tab", "Mod Değiştir"),
                key_badge("?", "AI"),
                key_badge(":clip", "Pano"),
                key_badge(":p", "Projeler"),
                key_badge(":e", "Emoji"),
                key_badge(":n", "Notlar"),
                key_badge("Esc", "Kapat"),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
        };

        let footer = row![
            user_pill,
            footer_shortcuts,
            Space::new(Length::Fill, Length::Shrink),
            quick_actions,
        ]
        .spacing(Spacing::MD)
        .align_y(Alignment::Center)
        .width(Length::Fill);

        // Wrap results in an ultra-minimalist scrollable container with custom thin scrollbar
        let results_scrollable = scrollable(results_column)
            .id(scrollable::Id::new(SCROLLABLE_ID))
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(4.0)
                    .scroller_width(4.0)
                    .margin(2.0),
            ))
            .style(move |_, _| scrollable::Style {
                container: container::Style::default(),
                vertical_rail: scrollable::Rail {
                    background: None,
                    border: iced::Border::default(),
                    scroller: scrollable::Scroller {
                        color: palette.border.to_iced(),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                    },
                },
                horizontal_rail: scrollable::Rail {
                    background: None,
                    border: iced::Border::default(),
                    scroller: scrollable::Scroller {
                        color: iced::Color::TRANSPARENT,
                        border: iced::Border::default(),
                    },
                },
                gap: None,
            })
            .height(if self.mode == LauncherMode::DesktopMenu { Length::Shrink } else { Length::Fixed(320.0) })
            .width(if self.mode == LauncherMode::DesktopMenu { Length::Fill } else { Length::FillPortion(12) });

        // Right-hand Preview Pane (Raycast / macOS Spotlight style)
        let preview_pane: Element<'_, Self::Message> = match self.mode {
            LauncherMode::Applications => {
                let app_offset = if self.calc_result.is_some() { 1 } else { 0 };

                if let Some((expr, res)) = &self.calc_result {
                    if self.selected_index == 0 {
                        return container(
                            column![
                                Space::new(Length::Fill, Length::Fixed(16.0)),
                                text("🧮").size(56.0),
                                Space::new(Length::Fill, Length::Fixed(8.0)),
                                text(format!("= {}", res))
                                    .size(Typography::TITLE)
                                    .color(palette.primary.to_iced()),
                                text(format!("İfade: {}", expr))
                                    .size(Typography::BODY)
                                    .color(palette.text_muted.to_iced()),
                                Space::new(Length::Fill, Length::Fixed(16.0)),
                                text("↵ Enter: Sonucu panoya kopyala")
                                    .size(Typography::CAPTION)
                                    .color(palette.primary.to_iced()),
                            ]
                            .spacing(Spacing::XS)
                            .align_x(Alignment::Center)
                            .width(Length::Fill),
                        )
                        .padding(Spacing::MD)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .width(Length::FillPortion(9))
                        .height(Length::Fixed(320.0))
                        .into();
                    }
                }

                let app_idx = self.selected_index.saturating_sub(app_offset);
                if let Some(entry) = self.filtered_entries.get(app_idx) {
                    let icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = entry.icon_path {
                        iced::widget::image(image::Handle::from_path(icon_path))
                            .width(Length::Fixed(56.0))
                            .height(Length::Fixed(56.0))
                            .into()
                    } else {
                        let icon_glyph = app_icon_char(entry);
                        text(icon_glyph)
                            .font(LUCIDE_FONT)
                            .size(36.0)
                            .color(palette.primary.to_iced())
                            .into()
                    };

                    let icon_box = container(icon_view)
                        .width(Length::Fixed(72.0))
                        .height(Length::Fixed(72.0))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .padding(Spacing::SM)
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.surface.to_iced())),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::MD),
                            },
                            shadow: iced::Shadow::default(),
                            ..Default::default()
                        });

                    let mut details = column![
                        icon_box,
                        text(&entry.name)
                            .size(Typography::TITLE)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_x(Alignment::Center);

                    if let Some(generic) = &entry.generic_name {
                        details = details.push(
                            text(generic)
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        );
                    }

                    if let Some(comment) = &entry.comment {
                        details = details.push(
                            text(comment)
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        );
                    }

                    // Metadata block
                    let metadata_col = column![
                        row![
                            text("Çalıştır:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(&entry.exec)
                                .size(Typography::CAPTION)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(Spacing::XS),
                        row![
                            text("Kimlik:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(&entry.id)
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(Spacing::XS),
                        row![
                            text("Terminal:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(if entry.terminal { "Yes" } else { "No" })
                                .size(Typography::CAPTION)
                                .color(if entry.terminal {
                                    palette.warning.to_iced()
                                } else {
                                    palette.text_muted.to_iced()
                                }),
                        ]
                        .spacing(Spacing::XS),
                    ]
                    .spacing(Spacing::XXS);

                    let meta_container = container(metadata_col)
                        .padding(Spacing::SM)
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(
                                palette.surface_hover.to_iced(),
                            )),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                            ..Default::default()
                        });

                    container(
                        column![details, meta_container]
                            .spacing(Spacing::MD)
                            .align_x(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .padding(Spacing::SM)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    let file_idx = app_idx.saturating_sub(self.filtered_entries.len());
                    if let Some(file) = self.filtered_files.get(file_idx) {
                        let icon_glyph = if file.is_dir {
                            Icon::FOLDER
                        } else {
                            Icon::APP_WINDOW
                        };
                        let icon_box = container(
                            text(icon_glyph)
                                .font(LUCIDE_FONT)
                                .size(36.0)
                                .color(palette.primary.to_iced()),
                        )
                        .padding(Spacing::MD)
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.surface.to_iced())),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::MD),
                            },
                            shadow: iced::Shadow::default(),
                            ..Default::default()
                        });

                        let details = column![
                            icon_box,
                            text(&file.name)
                                .size(Typography::TITLE)
                                .color(palette.text_primary.to_iced()),
                            text(if file.is_dir { "Klasör" } else { "Dosya" })
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_x(Alignment::Center);

                        let metadata_col = column![row![
                            text("Yol:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(file.path.to_string_lossy().to_string())
                                .size(Typography::CAPTION)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(Spacing::XS),]
                        .spacing(Spacing::XXS);

                        let meta_container = container(metadata_col)
                            .padding(Spacing::SM)
                            .width(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(
                                    palette.surface_hover.to_iced(),
                                )),
                                border: iced::Border {
                                    color: palette.border.to_iced(),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                shadow: iced::Shadow::default(),
                                ..Default::default()
                            });

                        container(
                            column![details, meta_container]
                                .spacing(Spacing::MD)
                                .align_x(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .padding(Spacing::SM)
                        .width(Length::FillPortion(9))
                        .height(Length::Fixed(320.0))
                        .into()
                    } else {
                        container(
                            text("Öğe seçilmedi")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        )
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .width(Length::FillPortion(9))
                        .height(Length::Fixed(320.0))
                        .into()
                    }
                }
            }
            LauncherMode::Projects => {
                if let Some(proj) = self.filtered_projects.get(self.selected_index) {
                    let icon_box = container(
                        text(Icon::CODE)
                            .font(LUCIDE_FONT)
                            .size(36.0)
                            .color(palette.primary.to_iced()),
                    )
                    .padding(Spacing::MD)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.surface.to_iced())),
                        border: iced::Border {
                            color: palette.border.to_iced(),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::MD),
                        },
                        shadow: iced::Shadow::default(),
                        ..Default::default()
                    });

                    let details = column![
                        icon_box,
                        text(&proj.name)
                            .size(Typography::TITLE)
                            .color(palette.text_primary.to_iced()),
                        text(format!("{} Project", proj.project_type))
                            .size(Typography::BODY)
                            .color(palette.primary.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_x(Alignment::Center);

                    let git_badge_row = if let Some(ref branch) = proj.git_branch {
                        let dirty_str = match proj.git_dirty_count {
                            Some(0) | None => "✓ Temiz".to_string(),
                            Some(count) => format!("⚠️ {} değişiklik", count),
                        };
                        let dirty_color = match proj.git_dirty_count {
                            Some(0) | None => palette.success.to_iced(),
                            Some(_) => palette.warning.to_iced(),
                        };
                        row![
                            container(
                                row![
                                    text("🌿").size(10.0),
                                    text(branch.clone())
                                        .size(Typography::CAPTION)
                                        .color(palette.text_primary.to_iced()),
                                ]
                                .spacing(3)
                                .align_y(Alignment::Center)
                            )
                            .padding([2, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(0.2, 0.7, 0.4, 0.15))),
                                border: iced::Border {
                                    color: iced::Color::from_rgba(0.2, 0.7, 0.4, 0.35),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                ..Default::default()
                            }),
                            container(
                                text(dirty_str)
                                    .size(Typography::CAPTION)
                                    .color(dirty_color)
                            )
                            .padding([2, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                border: iced::Border {
                                    color: palette.border.to_iced(),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                ..Default::default()
                            }),
                        ]
                        .spacing(Spacing::XS)
                    } else {
                        row![]
                    };

                    let meta_col = column![
                        git_badge_row,
                        row![
                            text("Yol:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(proj.path.to_string_lossy())
                                .size(Typography::CAPTION)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(Spacing::XS),
                        row![
                            text("Git:")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            text(if proj.has_git {
                                "Initialized (.git)"
                            } else {
                                "Yok"
                            })
                            .size(Typography::CAPTION)
                            .color(if proj.has_git {
                                palette.success.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }),
                        ]
                        .spacing(Spacing::XS),
                    ]
                    .spacing(Spacing::XS);

                    let meta_container = container(meta_col)
                        .padding(Spacing::SM)
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(
                                palette.surface_hover.to_iced(),
                            )),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            shadow: iced::Shadow::default(),
                            ..Default::default()
                        });

                    let action_buttons = row![
                        button(
                            row![text("💻").size(11.0), text("IDE").size(Typography::CAPTION)]
                                .spacing(4)
                                .align_y(Alignment::Center)
                        )
                        .padding([4, 8])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .on_press(Message::LaunchSpecificProject(proj.clone())),
                        button(
                            row![text(">_").size(11.0), text("Terminal").size(Typography::CAPTION)]
                                .spacing(4)
                                .align_y(Alignment::Center)
                        )
                        .padding([4, 8])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(palette.surface.to_iced())),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        })
                        .on_press(Message::OpenProjectTerminal(proj.path.clone())),
                        button(
                            row![text("📁").size(11.0), text("Dosyalar").size(Typography::CAPTION)]
                                .spacing(4)
                                .align_y(Alignment::Center)
                        )
                        .padding([4, 8])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(palette.surface.to_iced())),
                            text_color: palette.text_primary.to_iced(),
                            border: iced::Border {
                                color: palette.border.to_iced(),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        })
                        .on_press(Message::OpenProjectFiles(proj.path.clone())),
                    ]
                    .spacing(Spacing::XS);

                    container(
                        column![details, meta_container, action_buttons]
                            .spacing(Spacing::MD)
                            .align_x(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .padding(Spacing::SM)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    container(
                        text("Geliştirici projesi seçilmedi")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    )
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                }
            }
            LauncherMode::Clipboard => {
                if let Some(entry) = self.filtered_clipboard.get(self.selected_index) {
                    let content_preview = scrollable(
                        text(&entry.preview)
                            .size(Typography::BODY)
                            .color(palette.text_primary.to_iced()),
                    )
                    .height(Length::Fixed(220.0))
                    .width(Length::Fill);

                    let info_row = row![
                        text(format!("{} chars", entry.preview.len()))
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                        text("↵ Kopyala ve yapıştır")
                            .size(Typography::CAPTION)
                            .color(palette.primary.to_iced()),
                    ]
                    .spacing(Spacing::MD);

                    container(
                        column![
                            row![
                                text(Icon::CLIPBOARD)
                                    .font(LUCIDE_FONT)
                                    .size(Typography::BODY)
                                    .color(palette.primary.to_iced()),
                                text("Pano İçeriği")
                                    .size(Typography::CAPTION)
                                    .color(palette.primary.to_iced()),
                            ]
                            .spacing(Spacing::XS)
                            .align_y(Alignment::Center),
                            container(content_preview)
                                .padding(Spacing::SM)
                                .width(Length::Fill)
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        palette.surface_hover.to_iced(),
                                    )),
                                    border: iced::Border {
                                        color: palette.border.to_iced(),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::SM),
                                    },
                                    shadow: iced::Shadow::default(),
                                    ..Default::default()
                                }),
                            info_row,
                        ]
                        .spacing(Spacing::SM)
                        .width(Length::Fill),
                    )
                    .padding(Spacing::SM)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    container(
                        text("Pano öğesi seçilmedi")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    )
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                }
            }
            LauncherMode::Emoji => {
                if let Some(emoji_item) = self.filtered_emojis.get(self.selected_index) {
                    let code_points = emoji_item
                        .emoji
                        .chars()
                        .map(|c| format!("U+{:04X}", c as u32))
                        .collect::<Vec<_>>()
                        .join(" ");

                    container(
                        column![
                            Space::new(Length::Fill, Length::Fixed(16.0)),
                            text(emoji_item.emoji).size(64.0),
                            Space::new(Length::Fill, Length::Fixed(12.0)),
                            text(emoji_item.name)
                                .size(Typography::TITLE_SM)
                                .color(palette.text_primary.to_iced()),
                            text(emoji_item.category)
                                .size(Typography::CAPTION)
                                .color(palette.primary.to_iced()),
                            Space::new(Length::Fill, Length::Fixed(8.0)),
                            container(
                                column![
                                    text(format!("Unicode: {}", code_points))
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                    text(format!("Etiketler: {}", emoji_item.keywords))
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(4)
                            )
                            .padding([8, 12])
                            .width(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04)
                                )),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                            Space::new(Length::Fill, Length::Fill),
                            text("Panoya Kopyalamak İçin ↵ Enter")
                                .size(Typography::CAPTION)
                                .color(palette.primary.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .padding(Spacing::MD)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    container(
                        text("Emoji seçilmedi")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    )
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                }
            }
            LauncherMode::Notes => {
                let trimmed = self.search_query.trim();
                let has_new_item = !trimmed.is_empty();

                if has_new_item && self.selected_index == 0 {
                    container(
                        column![
                            Space::new(Length::Fill, Length::Fixed(16.0)),
                            text("➕").size(48.0),
                            Space::new(Length::Fill, Length::Fixed(8.0)),
                            text("Yeni Not Oluştur")
                                .size(Typography::TITLE_SM)
                                .color(palette.primary.to_iced()),
                            text("Corten Hızlı Karalama Defteri")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                            Space::new(Length::Fill, Length::Fixed(12.0)),
                            container(
                                text(trimmed)
                                    .size(Typography::BODY)
                                    .color(palette.text_primary.to_iced())
                            )
                            .padding(12)
                            .width(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                            Space::new(Length::Fill, Length::Fill),
                            text("Enter: Notu Kaydet")
                                .size(Typography::CAPTION)
                                .color(palette.primary.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .padding(Spacing::MD)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    let note_idx = if has_new_item {
                        self.selected_index.saturating_sub(1)
                    } else {
                        self.selected_index
                    };

                    if let Some(note) = self.filtered_notes.get(note_idx) {
                        let clean = note.trim_start_matches("- ");
                        container(
                            column![
                                Space::new(Length::Fill, Length::Fixed(12.0)),
                                text("📝 Not İçeriği")
                                    .size(Typography::TITLE_SM)
                                    .color(palette.primary.to_iced()),
                                Space::new(Length::Fill, Length::Fixed(8.0)),
                                container(
                                    text(clean)
                                        .size(Typography::BODY)
                                        .color(palette.text_primary.to_iced())
                                )
                                .padding(12)
                                .width(Length::Fill)
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(Radius::SM),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                                Space::new(Length::Fill, Length::Fill),
                                row![
                                    text(format!("Uzunluk: {} karakter", clean.len()))
                                        .size(Typography::CAPTION)
                                        .color(palette.text_muted.to_iced()),
                                    Space::new(Length::Fill, Length::Shrink),
                                    text("↵ Panoya Kopyala")
                                        .size(Typography::CAPTION)
                                        .color(palette.primary.to_iced()),
                                ]
                                .width(Length::Fill)
                                .align_y(Alignment::Center),
                            ]
                            .spacing(Spacing::XS)
                            .align_x(Alignment::Center)
                            .width(Length::Fill),
                        )
                        .padding(Spacing::MD)
                        .width(Length::FillPortion(9))
                        .height(Length::Fixed(320.0))
                        .into()
                    } else {
                        container(
                            text("Bir not seçin veya yeni yazın")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        )
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .width(Length::FillPortion(9))
                        .height(Length::Fixed(320.0))
                        .into()
                    }
                }
            }
            LauncherMode::CheatSheet => {
                let shortcuts = self.filtered_shortcuts();
                if let Some(sc) = shortcuts.get(self.selected_index) {
                    let chords_row = row(sc.chords.split('+').map(|part| {
                        container(
                            text(part.trim())
                                .size(13.0)
                                .color(palette.primary.to_iced())
                        )
                        .padding([4, 10])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4),
                                width: 1.5,
                                radius: iced::border::Radius::from(6.0),
                            },
                            ..Default::default()
                        })
                        .into()
                    }))
                    .spacing(6)
                    .align_y(Alignment::Center);

                    container(
                        column![
                            Space::new(Length::Fill, Length::Fixed(16.0)),
                            chords_row,
                            Space::new(Length::Fill, Length::Fixed(8.0)),
                            text(sc.title)
                                .size(Typography::TITLE_SM)
                                .color(palette.text_primary.to_iced()),
                            container(
                                text(sc.category)
                                    .size(11.0)
                                    .color(palette.primary.to_iced())
                            )
                            .padding([2, 8])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(4.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                            Space::new(Length::Fill, Length::Fixed(12.0)),
                            text(sc.desc)
                                .size(Typography::BODY)
                                .color(palette.text_muted.to_iced()),
                            Space::new(Length::Fill, Length::Fixed(12.0)),
                            container(
                                text(format!("💡 İpucu: {}", sc.tip))
                                    .size(Typography::CAPTION)
                                    .color(palette.text_primary.to_iced())
                            )
                            .padding(10)
                            .width(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(6.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                            Space::new(Length::Fill, Length::Fill),
                            text("↵ Kapatmak için Enter veya Esc'ye basın")
                                .size(Typography::CAPTION)
                                .color(palette.text_disabled.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .padding(Spacing::MD)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                } else {
                    container(
                        text("Bir kısayol seçin")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    )
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .width(Length::FillPortion(9))
                    .height(Length::Fixed(320.0))
                    .into()
                }
            }
            LauncherMode::DesktopMenu => Space::new(Length::Shrink, Length::Shrink).into(),
        };

        // Vertical divider between list and preview pane
        let vertical_divider = container(row![])
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(320.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(palette.border.to_iced())),
                ..Default::default()
            });

        // Main view content: Spotlight split view vs KDE Application Dashboard Grid
        let (content_view, modal_width): (Element<'_, Self::Message>, f32) = match self.config.style
        {
            LauncherStyle::Spotlight => {
                let split: Element<'_, Self::Message> = if self.mode == LauncherMode::DesktopMenu {
                    row![results_scrollable].width(Length::Fill).into()
                } else {
                    row![results_scrollable, vertical_divider, preview_pane]
                        .spacing(Spacing::SM)
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .into()
                };
                (split, if self.mode == LauncherMode::DesktopMenu { 300.0 } else { (self.config.width as f32).max(740.0) })
            }
            LauncherStyle::Dashboard => {
                if self.mode != LauncherMode::Applications {
                    let split: Element<'_, Self::Message> =
                        row![results_scrollable, vertical_divider, preview_pane]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into();
                    (split, 980.0)
                } else {
                    // macOS Launchpad / GNOME App Grid clean 6-column expansive view
                    let mut grid_col = column![].spacing(Spacing::SM).width(Length::Fill);
                    let chunk_size = 6;
                    let chunks: Vec<_> = self.filtered_entries.chunks(chunk_size).collect();

                    for chunk in chunks {
                        let mut grid_row = row![].spacing(Spacing::SM).width(Length::Fill);
                        for entry in chunk {
                            let is_sel = self
                                .filtered_entries
                                .get(self.selected_index)
                                .map(|e| e.id == entry.id)
                                .unwrap_or(false);
                            let entry_clone = entry.clone();

                            let app_icon_view: Element<'_, Self::Message> = render_app_icon(entry, 44.0, palette.primary.to_iced());

                            let display_name = if entry.name.chars().count() > 18 {
                                format!("{}…", entry.name.chars().take(16).collect::<String>())
                            } else {
                                entry.name.clone()
                            };

                            let app_card = button(
                                column![
                                    container(app_icon_view)
                                        .width(Length::Fixed(52.0))
                                        .height(Length::Fixed(52.0))
                                        .center_x(Length::Fill)
                                        .center_y(Length::Fill)
                                        .style(move |_| container::Style {
                                            background: Some(iced::Background::Color(if is_sel {
                                                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.20)
                                            } else {
                                                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                                            })),
                                            border: iced::Border {
                                                radius: iced::border::Radius::from(Radius::MD),
                                                color: if is_sel { palette.primary.to_iced() } else { iced::Color::TRANSPARENT },
                                                width: if is_sel { 1.0 } else { 0.0 },
                                            },
                                            ..Default::default()
                                        }),
                                    text(display_name)
                                        .size(Typography::CAPTION)
                                        .color(palette.text_primary.to_iced()),
                                ]
                                .spacing(Spacing::XXS)
                                .align_x(Alignment::Center)
                                .width(Length::Fill),
                            )
                            .padding([10.0, 4.0])
                            .width(Length::FillPortion(1))
                            .height(Length::Fixed(96.0))
                            .style(move |_, status| {
                                let bg = match status {
                                    button::Status::Hovered => {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                                    }
                                    _ if is_sel => iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15),
                                    _ => iced::Color::TRANSPARENT,
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        color: if is_sel {
                                            palette.primary.to_iced()
                                        } else {
                                            iced::Color::TRANSPARENT
                                        },
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::MD),
                                    },
                                    shadow: iced::Shadow::default(),
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::LaunchSpecificApp(entry_clone));

                            grid_row = grid_row.push(app_card);
                        }
                        // Pad empty spots in last row
                        for _ in chunk.len()..chunk_size {
                            grid_row =
                                grid_row.push(Space::new(Length::FillPortion(1), Length::Shrink));
                        }
                        grid_col = grid_col.push(grid_row);
                    }

                    let dashboard_scroll = scrollable(grid_col)
                        .height(Length::Fixed(380.0))
                        .width(Length::Fill);

                    (dashboard_scroll.into(), 980.0)
                }
            }
            LauncherStyle::StartMenu => {
                let view = row![results_scrollable]
                    .width(Length::Fill);
                (view.into(), 480.0)
            }
        };

        // Modal container with rich Corten aesthetic, ambient shadow & border
        let col = column![mode_tabs, search_bar, category_pills, content_view, footer]
            .spacing(Spacing::SM)
            .width(Length::Fill);

        let modal = container(col)
            .padding(Spacing::MD)
            .width(Length::Fill)
            .max_width(modal_width)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09,
                    0.10,
                    0.12,
                    0.94 * entrance_alpha * content_alpha,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.45 * entrance_alpha,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::LG),
                },
                shadow: iced::Shadow::default(),
                ..Default::default()
            });

        let inner_modal = mouse_area(modal).on_press(Message::NoOp);
        let (align_y, padding) = (
            iced::alignment::Vertical::Center,
            iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 48.0,
                left: 0.0,
            },
        );

        mouse_area(
            container(inner_modal)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(align_y)
                .padding(padding),
        )
        .on_press(Message::CloseRequested)
        .into()
    }
}

impl CortenLauncherApp {
    fn current_count(&self) -> usize {
        match self.mode {
            LauncherMode::Applications => {
                if self.search_query.starts_with('>') {
                    return self.filtered_commands.len();
                }
                let calc_count = if self.calc_result.is_some() { 1 } else { 0 };
                calc_count + self.filtered_entries.len() + self.filtered_files.len()
            }
            LauncherMode::Projects => self.filtered_projects.len(),
            LauncherMode::Clipboard => self.filtered_clipboard.len(),
            LauncherMode::Emoji => self.filtered_emojis.len(),
            LauncherMode::Notes => {
                let add_new = if !self.search_query.trim().is_empty() { 1 } else { 0 };
                add_new + self.filtered_notes.len()
            }
            LauncherMode::CheatSheet => self.filtered_shortcuts().len(),
            LauncherMode::DesktopMenu => self.desktop_menu_items().len(),
        }
    }

    fn filter_current(&mut self) {
        match self.mode {
            LauncherMode::Applications => self.filter_entries(),
            LauncherMode::Projects => self.filter_projects(),
            LauncherMode::Clipboard => self.filter_clipboard(),
            LauncherMode::Emoji => self.filter_emojis(),
            LauncherMode::Notes => self.filter_notes(),
            LauncherMode::CheatSheet | LauncherMode::DesktopMenu => {}
        }
    }

    fn load_notes_entries(&mut self) {
        self.notes_entries = read_notes_file();
        self.notes_entries.reverse();
        self.filter_notes();
    }

    fn filter_notes(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            self.filtered_notes = self.notes_entries.clone();
        } else {
            self.filtered_notes = self
                .notes_entries
                .iter()
                .filter(|n| n.to_lowercase().contains(&query))
                .cloned()
                .collect();
        }
    }

    fn filter_emojis(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            self.filtered_emojis = EMOJI_DATABASE.to_vec();
            return;
        }

        self.filtered_emojis = EMOJI_DATABASE
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&query)
                    || e.keywords.to_lowercase().contains(&query)
                    || e.category.to_lowercase().contains(&query)
                    || e.emoji.contains(&query)
            })
            .cloned()
            .collect();
    }

    fn filter_projects(&mut self) {
        if self.search_query.trim().is_empty() {
            self.filtered_projects = self
                .all_projects
                .iter()
                .take(self.config.max_results)
                .cloned()
                .collect();
            return;
        }

        let mut scored: Vec<(u32, DevProject)> = Vec::new();
        let mut utf32_buf = Vec::new();
        for proj in &self.all_projects {
            let haystack = format!(
                "{} {} {}",
                proj.name,
                proj.project_type,
                proj.path.display()
            );
            if let Some(score) = nucleo::pattern::Pattern::parse(
                &self.search_query,
                nucleo::pattern::CaseMatching::Ignore,
                nucleo::pattern::Normalization::Smart,
            )
            .score(
                nucleo::Utf32Str::new(&haystack, &mut utf32_buf),
                &mut self.matcher,
            ) {
                scored.push((score, proj.clone()));
            }
        }

        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        self.filtered_projects = scored
            .into_iter()
            .take(self.config.max_results)
            .map(|(_, proj)| proj)
            .collect();
    }

    fn load_clipboard(&mut self) {
        if let Ok(entries) = ClipboardManager::list_entries() {
            self.clipboard_entries = entries;
        } else {
            self.clipboard_entries.clear();
        }
        self.filter_clipboard();
    }

    fn filter_clipboard(&mut self) {
        if self.search_query.trim().is_empty() {
            self.filtered_clipboard = self
                .clipboard_entries
                .iter()
                .take(self.config.max_results)
                .cloned()
                .collect();
            return;
        }

        let mut scored: Vec<(u32, ClipboardEntry)> = Vec::new();
        let mut utf32_buf = Vec::new();
        for entry in &self.clipboard_entries {
            if let Some(score) = nucleo::pattern::Pattern::parse(
                &self.search_query,
                nucleo::pattern::CaseMatching::Ignore,
                nucleo::pattern::Normalization::Smart,
            )
            .score(
                nucleo::Utf32Str::new(&entry.preview, &mut utf32_buf),
                &mut self.matcher,
            ) {
                scored.push((score, entry.clone()));
            }
        }

        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        self.filtered_clipboard = scored
            .into_iter()
            .take(self.config.max_results)
            .map(|(_, entry)| entry)
            .collect();
    }

    fn filter_entries(&mut self) {
        if self.search_query.starts_with('>') {
            let cmd_query = self.search_query.trim_start_matches('>').trim().to_lowercase();
            self.filtered_commands = QUICK_COMMANDS
                .iter()
                .filter(|c| {
                    cmd_query.is_empty()
                        || c.name.to_lowercase().contains(&cmd_query)
                        || c.description.to_lowercase().contains(&cmd_query)
                })
                .cloned()
                .collect();
            self.filtered_entries.clear();
            self.filtered_files.clear();
            self.calc_result = None;
            return;
        } else {
            self.filtered_commands.clear();
        }

        self.calc_result = parse_color_query(&self.search_query).or_else(|| {
            eval_math_expression(&self.search_query)
                .map(|val| (self.search_query.clone(), format_calc_result(val)))
        });

        if self.search_query.trim().len() >= 2 {
            self.filtered_files = find_matching_files(&self.search_query, 6);
        } else {
            self.filtered_files.clear();
        }

        let category_filtered: Vec<AppEntry> = self
            .all_entries
            .iter()
            .filter(|entry| match self.selected_category {
                AppCategory::All => true,
                AppCategory::Internet => {
                    let cat = entry.categories.join(" ").to_lowercase();
                    let name = entry.name.to_lowercase();
                    cat.contains("network")
                        || cat.contains("web")
                        || name.contains("browser")
                        || name.contains("firefox")
                        || name.contains("chrome")
                }
                AppCategory::Office => {
                    let cat = entry.categories.join(" ").to_lowercase();
                    let name = entry.name.to_lowercase();
                    cat.contains("office")
                        || cat.contains("text")
                        || cat.contains("document")
                        || name.contains("calc")
                        || name.contains("pdf")
                }
                AppCategory::Media => {
                    let cat = entry.categories.join(" ").to_lowercase();
                    let name = entry.name.to_lowercase();
                    cat.contains("audio")
                        || cat.contains("video")
                        || cat.contains("media")
                        || cat.contains("music")
                        || name.contains("vlc")
                        || name.contains("player")
                }
                AppCategory::Development => {
                    let cat = entry.categories.join(" ").to_lowercase();
                    let name = entry.name.to_lowercase();
                    cat.contains("development")
                        || cat.contains("ide")
                        || name.contains("code")
                        || name.contains("editor")
                        || name.contains("git")
                }
                AppCategory::System => {
                    let cat = entry.categories.join(" ").to_lowercase();
                    let name = entry.name.to_lowercase();
                    cat.contains("system")
                        || cat.contains("settings")
                        || cat.contains("utility")
                        || name.contains("terminal")
                        || name.contains("config")
                }
            })
            .cloned()
            .collect();

        if self.search_query.trim().is_empty() {
            let limit = if self.config.style == LauncherStyle::Dashboard { 200 } else if self.config.style == LauncherStyle::StartMenu { 120 } else { self.config.max_results };
            self.filtered_entries = category_filtered
                .into_iter()
                .take(limit)
                .collect();
            return;
        }

        let mut scored: Vec<(u32, AppEntry)> = Vec::new();
        let mut utf32_buf = Vec::new();

        for entry in category_filtered {
            let haystack = format!("{} {}", entry.name, entry.exec);
            if let Some(score) = nucleo::pattern::Pattern::parse(
                &self.search_query,
                nucleo::pattern::CaseMatching::Ignore,
                nucleo::pattern::Normalization::Smart,
            )
            .score(
                nucleo::Utf32Str::new(&haystack, &mut utf32_buf),
                &mut self.matcher,
            ) {
                scored.push((score, entry));
            }
        }

        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        self.filtered_entries = scored
            .into_iter()
            .take(self.config.max_results)
            .map(|(_, entry)| entry)
            .collect();
    }

    pub fn build_shortcuts(&self) -> Vec<ShortcutEntry> {
        let t = self.language.t();
        vec![
            // Windows
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + Return",
                title: t.shortcut_terminal,
                desc: "Hızlı GPU-hızlandırmalı terminal penceresi açar",
                tip: "Super+Shift+Return ile aktif pencereyi ana (master) pencere yapabilirsiniz.",
                icon_char: Icon::TERMINAL,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + Q",
                title: t.shortcut_close,
                desc: "Odaklanmış aktif pencereyi kapatır",
                tip: "Acil TTY çıkışı için Super+Shift+Escape kullanabilirsiniz.",
                icon_char: Icon::X,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + F",
                title: t.shortcut_fullscreen,
                desc: "Aktif pencereyi tam ekran yapar veya önceki boyutuna döner",
                tip: "Diğer ekrana taşımak için Super+Shift+Right / Left kullanın.",
                icon_char: Icon::MONITOR,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + W",
                title: t.shortcut_overview,
                desc: "Tüm pencereleri Exposé genel bakış grid ızgarasında gösterir",
                tip: "Touchpad üzerinde 3 parmağınızı yukarı kaydırarak da açabilirsiniz.",
                icon_char: Icon::APP_WINDOW,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + Shift + Space",
                title: "Yüzen (Floating) Mod",
                desc: "Pencereyi serbest kayan / yüzen konuma geçirir",
                tip: "Pencereyi taşımak için Super tuşuna basılı tutup sol fareyle sürükleyin.",
                icon_char: Icon::APP_WINDOW,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + Tab",
                title: "Pencere Yerleşim Algoritması Döngüsü",
                desc: "Master-Stack, Spiral ve Grid algoritmaları arasında geçiş yapar",
                tip: "Alt+Tab pencereler arası odak değiştirir.",
                icon_char: Icon::APP_WINDOW,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + J / K",
                title: "Sonraki / Önceki Pencereye Odaklan",
                desc: "Fareye dokunmadan pencereler arasında geçiş yapar",
                tip: "Super+Shift+J/K pencerelerin yerini fiziksel olarak değiştirir (swap).",
                icon_char: Icon::CHEVRON_RIGHT,
            },
            ShortcutEntry {
                category: t.cat_windows,
                chords: "Super + H / L",
                title: "Master Pencere Oranını Ayarla",
                desc: "Ana pencere genişliğini %5 oranında artırır veya azaltır",
                tip: "İki pencere yan yanayken kod editörüne daha çok alan kazandırır.",
                icon_char: Icon::CODE,
            },
            // Workspaces
            ShortcutEntry {
                category: "Çalışma Alanları",
                chords: "Super + 1..9",
                title: "Çalışma Alanına Geç",
                desc: "1: Kod, 2: Web, 3: Sohbet, 4: Medya sanal alanlarına atlar",
                tip: "Aktif pencereyi çalışma alanına taşımak için Super+Shift+1..9 kullanın.",
                icon_char: Icon::FOLDER,
            },
            ShortcutEntry {
                category: "Çalışma Alanları",
                chords: "3 Parmak Yatay Kaydırma",
                title: "Touchpad: Çalışma Alanı Değiştir",
                desc: "Dokunmatik yüzeyde 3 parmakla sola/sağa kaydırarak geçiş yapın",
                tip: "Doğal kaydırma fiziksel olarak Smithay Wayland motorundan okunur.",
                icon_char: Icon::MONITOR,
            },
            ShortcutEntry {
                category: "Çalışma Alanları",
                chords: "3 Parmak Yukarı Kaydırma",
                title: "Touchpad: Pencere Genel Bakışı (Exposé)",
                desc: "Dokunmatik yüzeyde 3 parmakla yukarı kaydırarak tüm pencereleri görün",
                tip: "Pencereye tıklayarak doğrudan o pencereye odaklanabilirsiniz.",
                icon_char: Icon::APP_WINDOW,
            },
            ShortcutEntry {
                category: "Çalışma Alanları",
                chords: "4 Parmak Aşağı Kaydırma",
                title: "Touchpad: Masaüstünü Göster",
                desc: "Pencereleri gizleyip masaüstü duvar kâğıdını gösterir",
                tip: "Tekrar kaydırarak pencerelerinizi geri getirebilirsiniz.",
                icon_char: Icon::MONITOR,
            },
            // System
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + Space",
                title: t.shortcut_launcher,
                desc: "Spotlight başlatıcı, hesap makinesi, dosya arama ve AI sorguları",
                tip: "? yazarak doğrudan Corten AI'a doğal dille komut verebilirsiniz.",
                icon_char: Icon::SEARCH,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + X",
                title: t.shortcut_power,
                desc: "Güç & Oturum Merkezi (Kapat, Yeniden Başlat, Askıya Al, Kilitle)",
                tip: "Onay koruması sayesinde kazara kapanmaları önler.",
                icon_char: Icon::POWER,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + ?",
                title: t.cheat_sheet_title,
                desc: "Bu akrilik klavye kısayolları ve jestler rehberini açar",
                tip: "Super+F1 veya 'corten-launcher --cheat-sheet' ile de açılabilir.",
                icon_char: Icon::KEYBOARD,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + E",
                title: t.shortcut_files,
                desc: "Corten Dosya Yöneticisi'ni açar",
                tip: "Geliştirici projelerine hızlı atlamak için Spotlight'ta :p yazın.",
                icon_char: Icon::FOLDER,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + I",
                title: t.shortcut_settings,
                desc: "Corten Sistem & Donanım Ayarları merkezini açar",
                tip: "Ekran çözünürlüğü, gece ışığı ve dil tercihleri buradan yönetilir.",
                icon_char: Icon::CODE,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + Escape",
                title: "Görev & Donanım Yöneticisi",
                desc: "Corten Monitor CPU, RAM, GPU ve süreç kullanımını gösterir",
                tip: "Arka plan süreçlerini tek tıkla sonlandırabilirsiniz.",
                icon_char: Icon::CPU,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "PrintScreen / Super+Shift+S",
                title: "Ekran Görüntüsü Al",
                desc: "PrintScreen tüm ekranı, Super+Shift+S seçili bölgeyi yakalar",
                tip: "Görüntü doğrudan ~/Pictures klasörüne kaydedilir ve panoya kopyalanır.",
                icon_char: Icon::CLIPBOARD,
            },
            ShortcutEntry {
                category: t.cat_system,
                chords: "Super + Ctrl + L",
                title: "Ekranı Kilitle",
                desc: "PAM ve fprintd parmak izi korumalı Corten Lock ekranını açar",
                tip: "Güvenlik için bilgisayarın başından ayrılırken kilitlemeyi unutmayın.",
                icon_char: Icon::LOCK,
            },
            // AI
            ShortcutEntry {
                category: "Yapay Zekâ & Sentinel",
                chords: "Super + Alt + Space",
                title: "Corten AI Asistanı & Gözetim HUD",
                desc: "Ekrandaki içeriği analiz eden yerel veya bulut yapay zekâ asistanı",
                tip: "Sıfır veri sızıntısıyla Ollama üzerinde tamamen yerel çalışır.",
                icon_char: Icon::SHIELD,
            },
        ]
    }

    pub fn filtered_shortcuts(&self) -> Vec<ShortcutEntry> {
        let query = self.search_query.trim().to_lowercase();
        let all = self.build_shortcuts();
        if query.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|s| {
                    s.chords.to_lowercase().contains(&query)
                        || s.title.to_lowercase().contains(&query)
                        || s.desc.to_lowercase().contains(&query)
                        || s.category.to_lowercase().contains(&query)
                })
                .collect()
        }
    }

    pub fn view_desktop_context_menu(
        &self,
        palette: Palette,
        entrance_alpha: f32,
        content_alpha: f32,
    ) -> Element<'_, Message> {
        let mut menu_col = column![].spacing(2).width(Length::Fill);

        let items = self.desktop_menu_items();
        let total_len = items.len();

        for (i, item) in items.into_iter().enumerate() {
            let is_selected = self.selected_index == i;
            let (bg, border_color) = if is_selected {
                (palette.surface_hover.to_iced(), palette.primary.to_iced())
            } else {
                (iced::Color::TRANSPARENT, iced::Color::TRANSPARENT)
            };

            let keycap: Element<'_, Message> = if !item.shortcut.is_empty() {
                container(
                    text(item.shortcut)
                        .size(10.0)
                        .font(iced::Font::MONOSPACE)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([2, 5])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                })
                .into()
            } else {
                Space::new(Length::Shrink, Length::Shrink).into()
            };

            let row_item = button(
                row![
                    text(item.icon_char)
                        .font(LUCIDE_FONT)
                        .size(15.0)
                        .color(palette.primary.to_iced()),
                    Space::new(Length::Fixed(8.0), Length::Shrink),
                    text(item.title)
                        .size(12.5)
                        .color(palette.text_primary.to_iced())
                        .width(Length::Fill),
                    keycap,
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .padding([5, 8]),
            )
            .width(Length::Fill)
            .style(move |_, status| {
                let item_bg = match status {
                    button::Status::Hovered => palette.surface_hover.to_iced(),
                    _ if is_selected => palette.surface_hover.to_iced(),
                    _ => bg,
                };
                button::Style {
                    background: Some(iced::Background::Color(item_bg)),
                    text_color: palette.text_primary.to_iced(),
                    border: iced::Border {
                        color: border_color,
                        width: if is_selected { 1.0 } else { 0.0 },
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    shadow: iced::Shadow::default(),
                }
            })
            .on_press(Message::SelectDesktopAction(item.action));

            menu_col = menu_col.push(row_item);

            // Subtle divider after file manager and before power hub
            if i == 2 || i == total_len.saturating_sub(2) {
                let divider = container(Space::new(Length::Fill, Length::Fixed(1.0)))
                    .padding([2, 4])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                        ..Default::default()
                    });
                menu_col = menu_col.push(divider);
            }
        }

        let menu_card = container(menu_col)
            .padding(6)
            .width(Length::Fixed(240.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09,
                    0.10,
                    0.12,
                    0.97 * entrance_alpha * content_alpha,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.50 * entrance_alpha,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.65),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

        let inner_modal = mouse_area(menu_card)
            .on_press(Message::NoOp)
            .on_right_press(Message::NoOp);

        let (pad_left, pad_top) = if let (Some(x), Some(y)) = (self.cursor_pos.0, self.cursor_pos.1) {
            ((x as f32).clamp(12.0, 1680.0), (y as f32).clamp(12.0, 720.0))
        } else {
            (200.0, 150.0)
        };

        mouse_area(
            container(inner_modal)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(iced::Padding {
                    top: pad_top,
                    right: 0.0,
                    bottom: 0.0,
                    left: pad_left,
                }),
        )
        .on_press(Message::CloseRequested)
        .on_right_press(Message::CloseRequested)
        .into()
    }

    pub fn desktop_menu_items(&self) -> Vec<DesktopMenuItem> {
        let t = self.language.t();
        vec![
            DesktopMenuItem {
                icon_char: Icon::FOLDER,
                title: "Yeni Klasör Oluştur",
                shortcut: "Ctrl+Shift+N",
                action: DesktopAction::NewFolder,
            },
            DesktopMenuItem {
                icon_char: Icon::TERMINAL,
                title: t.shortcut_terminal,
                shortcut: "Super+Return",
                action: DesktopAction::Terminal,
            },
            DesktopMenuItem {
                icon_char: Icon::FOLDER,
                title: "Dosya Yöneticisi",
                shortcut: "Super+E",
                action: DesktopAction::FileManager,
            },
            DesktopMenuItem {
                icon_char: Icon::MONITOR,
                title: "Sonraki Duvar Kâğıdı",
                shortcut: "Super+Alt+W",
                action: DesktopAction::NextWallpaper,
            },
            DesktopMenuItem {
                icon_char: Icon::CPU,
                title: "Masaüstü Araçları",
                shortcut: "Super+D",
                action: DesktopAction::ToggleWidgets,
            },
            DesktopMenuItem {
                icon_char: Icon::MONITOR,
                title: "Görünüm & Tema",
                shortcut: "Super+W",
                action: DesktopAction::WallpaperAppearance,
            },
            DesktopMenuItem {
                icon_char: Icon::KEYBOARD,
                title: t.cheat_sheet_title,
                shortcut: "Super+?",
                action: DesktopAction::CheatSheet,
            },
            DesktopMenuItem {
                icon_char: Icon::APP_WINDOW,
                title: t.shortcut_settings,
                shortcut: "Super+,",
                action: DesktopAction::Settings,
            },
            DesktopMenuItem {
                icon_char: Icon::POWER,
                title: t.power_hub_title,
                shortcut: "Super+X",
                action: DesktopAction::PowerHub,
            },
        ]
    }

    fn execute_desktop_action(&mut self, action: DesktopAction) -> Task<Message> {
        match action {
            DesktopAction::NewFolder => {
                if let Ok(home) = std::env::var("HOME") {
                    let desktop = std::path::PathBuf::from(&home).join("Desktop");
                    let dir = if desktop.exists() {
                        desktop
                    } else {
                        std::path::PathBuf::from(&home)
                    };
                    let mut i = 1;
                    let mut p = dir.join("Yeni Klasör");
                    while p.exists() {
                        i += 1;
                        p = dir.join(format!("Yeni Klasör {}", i));
                    }
                    let _ = std::fs::create_dir_all(&p);
                    self.status_message = Some("Yeni klasör oluşturuldu".into());
                }
            }
            DesktopAction::Terminal => {
                let _ = launch_fallback_chain("corten-terminal || alacritty || foot || kitty || pterm || xterm || bash");
            }
            DesktopAction::FileManager => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/home".into());
                let _ = launch_fallback_chain(&format!(
                    "corten-files || nautilus || thunar || pcmanfm || dolphin || xdg-open {}",
                    home
                ));
            }
            DesktopAction::NextWallpaper => {
                let _ = Command::new("killall")
                    .args(["-USR1", "corten-wallpaper"])
                    .spawn();
            }
            DesktopAction::ToggleWidgets => {
                let _ = Command::new("pkill")
                    .args(["-USR1", "corten-widgets"])
                    .spawn();
            }
            DesktopAction::WallpaperAppearance => {
                let _ = launch_fallback_chain("corten-settings --tab appearance || corten-welcome");
            }
            DesktopAction::CheatSheet => {
                self.mode = LauncherMode::CheatSheet;
                self.selected_index = 0;
                self.search_query.clear();
                return Task::none();
            }
            DesktopAction::Settings => {
                let _ = launch_fallback_chain("corten-settings || gnome-control-center");
            }
            DesktopAction::PowerHub => {
                let _ = Command::new("corten-panel").arg("--power-hub").spawn();
            }
        }
        if !self.stay_open {
            self.should_exit = true;
        }
        Task::none()
    }
}

fn notes_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(home).join(".local").join("share").join("corten");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("notes.md")
}

fn read_notes_file() -> Vec<String> {
    let path = notes_file_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

fn append_note_file(text: &str) {
    let path = notes_file_path();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let timestamp = format!("{:02}:{:02}", hours, mins);
    let line = format!("- [{}] {}\n", timestamp, text.trim());
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_binary_system_and_paths() {
        assert!(resolve_binary("sh").is_some());
        assert!(resolve_binary("non_existent_binary_12345_xyz").is_none());
    }

    #[test]
    fn test_utf8_multibyte_fuzzy_matcher() {
        let mut matcher = nucleo::Matcher::default();
        let mut utf32_buf = Vec::new();

        let pattern = nucleo::pattern::Pattern::parse(
            "şifre",
            nucleo::pattern::CaseMatching::Ignore,
            nucleo::pattern::Normalization::Smart,
        );

        let haystack = "Kullanıcı Şifre Yöneticisi 🔐";
        let score = pattern.score(
            nucleo::Utf32Str::new(haystack, &mut utf32_buf),
            &mut matcher,
        );
        assert!(score.is_some(), "Multi-byte Turkish UTF-8 and emoji should score successfully");
    }
}
