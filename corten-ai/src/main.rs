use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::info;

use corten_config::{AiSettingsConfig, AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::input_style;
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES, NOTO_SANS_SEMIBOLD, Palette,
    PatinaVariant, Radius, Spacing, ThemeMode, Typography,
};
use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::{
    button, column, container, horizontal_space, row, scrollable, text, text_input, vertical_space,
};
use iced::{Alignment, Element, Event, Length, Subscription, event};
use iced_layershell::Application;
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_runtime::Task;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

mod engine;

use engine::{AiEngine, BackendType, ChatMessage, ChatMode, MessageRole, StreamChunk};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                println!(
                    "Corten AI — Yerel Yapay Zeka Asistanı (UDS / AURA-CORE / Ollama / OpenAI)"
                );
                println!("Kullanım:");
                println!("  corten-ai                  Masaüstü HUD arayüzünü başlatır (Wayland)");
                println!(
                    "  corten-ai --ask <SORGU>    Terminal üzerinden doğrudan sorgu çalıştırır"
                );
                println!("  corten-ai --prompt <SORGU> HUD arayüzünü belirtilen sorguyla açar");
                println!(
                    "  corten-ai --audit <DOSYA>  Belirtilen dosyayı güvenlik denetimiyle açar"
                );
                println!("  corten-ai --daemon         AURA-CORE UDS arka plan servisini başlatır");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("corten-ai {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--ask" | "-q" if args.len() > 2 => {
                let query = args[2..].join(" ");
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(async move {
                    let ai_cfg = ConfigManager::<AiSettingsConfig>::load_or_default("ai")
                        .map(|c| c.get().clone())
                        .unwrap_or_default();
                    let backend = AiEngine::auto_detect_with_config(Some(&ai_cfg)).await;
                    let (tx, mut rx) = unbounded_channel();
                    let cancel = Arc::new(AtomicBool::new(false));
                    let history = vec![ChatMessage {
                        role: MessageRole::User,
                        content: query.clone(),
                        thought: None,
                    }];
                    AiEngine::stream_query(backend, ChatMode::Chat, history, query, cancel, tx)
                        .await;
                    while let Some(chunk) = rx.recv().await {
                        match chunk {
                            StreamChunk::Token(tok) => print!("{tok}"),
                            StreamChunk::Thought(_) | StreamChunk::Metrics(_) => {}
                            StreamChunk::Error(err) => eprintln!("\nHata: {err}"),
                            StreamChunk::Done => {
                                println!();
                                break;
                            }
                        }
                    }
                });
                return Ok(());
            }
            "--daemon" => {
                info!("Starting Corten AI UDS daemon...");
                match AiEngine::launch_aura_daemon_uds() {
                    Ok(sock) => info!("AURA-CORE UDS daemon launched at {}", sock.display()),
                    Err(e) => eprintln!("AURA-CORE başlatılamadı: {e}"),
                }
                return Ok(());
            }
            _ => {}
        }
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!(
            "Hata: Aktif bir Wayland oturumu (WAYLAND_DISPLAY) bulunamadı.\n\
            Terminal üzerinden sorgu çalıştırmak için: corten-ai --ask \"<sorgunuz>\""
        );
        return Ok(());
    }

    info!("Starting Corten AI HUD (corten-ai)...");

    let initial_query = if args.len() > 1 && args[1] == "--prompt" && args.len() > 2 {
        Some(args[2..].join(" "))
    } else if args.len() > 1 && args[1] == "--audit" && args.len() > 2 {
        let file_path = args[2..].join(" ");
        let content = std::fs::read_to_string(&file_path)
            .unwrap_or_else(|_| "Dosya açılamadı veya ikili (binary) formatta.".to_string());
        let preview = content.lines().take(80).collect::<Vec<_>>().join("\n");
        Some(format!(
            "Aşağıdaki betik/kod dosyasını bir siber güvenlik uzmanı olarak analiz et. Dosya yolu: {}.\n\
            Ağ bağlantıları (curl, socket, IP), şüpheli dosya erişimleri (~/.ssh, /etc), yetki yükseltme (sudo), gizli kod veya tehdit var mı kontrol et ve Türkçe kısa bir güvenlik değerlendirme raporu sun:\n\n```\n{}\n```",
            file_path, preview
        ))
    } else {
        None
    };

    let flags = AppFlags { initial_query };

    CortenAiApp::run(Settings {
        id: Some("corten-ai".into()),
        flags,
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
            Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Right,
            layer: Layer::Top,
            margin: (44, 12, 12, 0),
            exclusive_zone: 0,
            size: Some((500, 0)),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(13.5),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

pub struct AppFlags {
    pub initial_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckLayoutMode {
    SideDeck,
    FloatingHub,
}

#[derive(Debug, Clone)]
pub enum Message {
    InitCompleted(BackendType, Option<String>),
    QueryInputChanged(String),
    SubmitQuery,
    StopStreaming,
    QuickAction(String),
    RefreshBackend,
    BackendDetected(BackendType),
    ToggleModelPicker,
    SelectModel(String),
    ModelSwitched(BackendType),
    SetChatMode(ChatMode),
    LaunchAuraDaemon,
    AuraDaemonLaunchResult(Result<String, String>),
    ToggleDeckMode,
    StreamStarted(Arc<Mutex<UnboundedReceiver<StreamChunk>>>),
    StreamChunkReceived(StreamChunk),
    ToggleThought(usize),
    ToggleActiveThought,
    ClearChat,
    CopyCode(String, usize),
    ResetCopyStatus,
    CursorTick,
    CloseRequested,
    EventTriggered(Event),
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

pub struct CortenAiApp {
    backend: BackendType,
    ai_config: AiSettingsConfig,
    chat_mode: ChatMode,
    show_model_picker: bool,
    history: Vec<ChatMessage>,
    active_stream_content: String,
    active_stream_thought: String,
    stream_rx: Option<Arc<Mutex<UnboundedReceiver<StreamChunk>>>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    live_tok_per_sec: Option<f32>,
    status_notice: Option<String>,
    query_input: String,
    is_loading: bool,
    is_launching_daemon: bool,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    copied_index: Option<usize>,
    cursor_blink: bool,
    expanded_thoughts: HashSet<usize>,
    active_thought_expanded: bool,
    layout_mode: DeckLayoutMode,
}

impl Application for CortenAiApp {
    type Message = Message;
    type Flags = AppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        let ai_config = ConfigManager::<AiSettingsConfig>::load_or_default("ai")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let initial_query = flags.initial_query;

        let app = Self {
            backend: BackendType::Offline,
            ai_config: ai_config.clone(),
            chat_mode: ChatMode::Chat,
            show_model_picker: false,
            history: Vec::new(),
            active_stream_content: String::new(),
            active_stream_thought: String::new(),
            stream_rx: None,
            cancel_flag: None,
            live_tok_per_sec: None,
            status_notice: None,
            query_input: initial_query.clone().unwrap_or_default(),
            is_loading: initial_query.is_some(),
            is_launching_daemon: false,
            theme_mode: ThemeMode::Dark,
            active_patina,
            copied_index: None,
            cursor_blink: true,
            expanded_thoughts: HashSet::new(),
            active_thought_expanded: false,
            layout_mode: DeckLayoutMode::SideDeck,
        };

        let init_task = Task::perform(
            async move {
                let detected = AiEngine::auto_detect_with_config(Some(&ai_config)).await;
                (detected, initial_query)
            },
            |(detected, opt_q)| Message::InitCompleted(detected, opt_q),
        );

        (app, init_task)
    }

    fn namespace(&self) -> String {
        "corten-ai".into()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        let pal = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: pal.text_primary.to_iced(),
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::InitCompleted(detected, opt_q) => {
                self.backend = detected;
                if !self.backend.supports_agent_mode() {
                    self.chat_mode = ChatMode::Chat;
                }
                if let Some(q) = opt_q {
                    self.is_loading = false;
                    self.query_input = q;
                    return Task::done(Message::SubmitQuery);
                }
            }
            Message::ToggleThought(idx) => {
                if self.expanded_thoughts.contains(&idx) {
                    self.expanded_thoughts.remove(&idx);
                } else {
                    self.expanded_thoughts.insert(idx);
                }
            }
            Message::ToggleActiveThought => {
                self.active_thought_expanded = !self.active_thought_expanded;
            }
            Message::StopStreaming => {
                if self.is_loading {
                    if let Some(flag) = self.cancel_flag.take() {
                        flag.store(true, Ordering::Relaxed);
                    }
                    self.is_loading = false;
                    self.stream_rx = None;
                    let thought = if self.active_stream_thought.trim().is_empty() {
                        None
                    } else {
                        Some(self.active_stream_thought.clone())
                    };
                    let mut content = self.active_stream_content.clone();
                    if !content.trim().is_empty() {
                        content.push_str("\n\n*(Durduruldu)*");
                        self.history.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content,
                            thought,
                        });
                    }
                    self.active_stream_content.clear();
                    self.active_stream_thought.clear();
                }
            }
            Message::QueryInputChanged(val) => {
                self.query_input = val;
            }
            Message::RefreshBackend => {
                self.status_notice = Some("Yerel sunucular taranıyor...".into());
                let cfg = self.ai_config.clone();
                return Task::perform(
                    async move { AiEngine::auto_detect_with_config(Some(&cfg)).await },
                    Message::BackendDetected,
                );
            }
            Message::BackendDetected(b) => {
                self.is_launching_daemon = false;
                self.status_notice = None;
                self.backend = b;
                if !self.backend.supports_agent_mode() {
                    self.chat_mode = ChatMode::Chat;
                }
            }
            Message::ToggleModelPicker => {
                if !self.backend.models().is_empty() {
                    self.show_model_picker = !self.show_model_picker;
                }
            }
            Message::SelectModel(model_id) => {
                self.show_model_picker = false;
                self.ai_config.selected_model = model_id.clone();
                if let Ok(mut mgr) = ConfigManager::<AiSettingsConfig>::load_or_default("ai") {
                    let _ = mgr.update(self.ai_config.clone());
                }
                self.status_notice = Some(format!("Model değiştiriliyor: {model_id}"));
                let backend = self.backend.clone();
                return Task::perform(
                    async move { AiEngine::switch_model(backend, model_id).await },
                    Message::ModelSwitched,
                );
            }
            Message::ModelSwitched(updated) => {
                self.backend = updated;
                self.status_notice = None;
            }
            Message::SetChatMode(mode) => {
                if mode == ChatMode::Agent && !self.backend.supports_agent_mode() {
                    self.status_notice =
                        Some("Ajan modu yalnızca AURA-CORE bağlıyken kullanılabilir.".into());
                } else {
                    self.chat_mode = mode;
                    self.status_notice = None;
                }
            }
            Message::LaunchAuraDaemon => {
                if !self.is_launching_daemon {
                    self.is_launching_daemon = true;
                    self.status_notice = Some("AURA-CORE (UDS) başlatılıyor...".into());
                    return Task::perform(
                        async move {
                            let res = AiEngine::launch_aura_daemon_uds()
                                .map(|p| p.to_string_lossy().to_string());
                            if res.is_ok() {
                                tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                            }
                            res
                        },
                        Message::AuraDaemonLaunchResult,
                    );
                }
            }
            Message::AuraDaemonLaunchResult(res) => match res {
                Ok(_) => {
                    return Task::done(Message::RefreshBackend);
                }
                Err(err) => {
                    self.is_launching_daemon = false;
                    self.status_notice = Some(err);
                }
            },
            Message::ToggleDeckMode => {
                self.layout_mode = match self.layout_mode {
                    DeckLayoutMode::SideDeck => DeckLayoutMode::FloatingHub,
                    DeckLayoutMode::FloatingHub => DeckLayoutMode::SideDeck,
                };
                return match self.layout_mode {
                    DeckLayoutMode::SideDeck => Task::batch(vec![
                        Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                            Anchor::Top | Anchor::Bottom | Anchor::Right,
                        ))),
                        Task::done(Message::LayerAction(LayershellCustomActions::MarginChange(
                            (44, 12, 12, 0),
                        ))),
                        Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((
                            500, 0,
                        )))),
                    ]),
                    DeckLayoutMode::FloatingHub => Task::batch(vec![
                        Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                            Anchor::empty(),
                        ))),
                        Task::done(Message::LayerAction(LayershellCustomActions::MarginChange(
                            (0, 0, 0, 0),
                        ))),
                        Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((
                            760, 640,
                        )))),
                    ]),
                };
            }
            Message::ClearChat => {
                if let Some(flag) = self.cancel_flag.take() {
                    flag.store(true, Ordering::Relaxed);
                }
                self.history.clear();
                self.active_stream_content.clear();
                self.active_stream_thought.clear();
                self.expanded_thoughts.clear();
                self.live_tok_per_sec = None;
                self.is_loading = false;
                self.stream_rx = None;
            }
            Message::SubmitQuery => {
                let q = self.query_input.trim().to_string();
                if !q.is_empty() && !self.is_loading {
                    self.is_loading = true;
                    self.show_model_picker = false;
                    self.status_notice = None;
                    self.live_tok_per_sec = None;
                    self.active_stream_content.clear();
                    self.active_stream_thought.clear();
                    self.query_input.clear();

                    self.history.push(ChatMessage {
                        role: MessageRole::User,
                        content: q.clone(),
                        thought: None,
                    });

                    let backend = self.backend.clone();
                    let mode = self.chat_mode;
                    let history = self.history.clone();
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.cancel_flag = Some(Arc::clone(&cancel));

                    return Task::perform(
                        async move {
                            let (tx, rx) = unbounded_channel();
                            tokio::spawn(async move {
                                AiEngine::stream_query(backend, mode, history, q, cancel, tx).await;
                            });
                            Arc::new(Mutex::new(rx))
                        },
                        Message::StreamStarted,
                    );
                }
            }
            Message::QuickAction(action_text) => {
                if !self.is_loading {
                    self.query_input = action_text;
                    return Task::done(Message::SubmitQuery);
                }
            }
            Message::StreamStarted(rx_arc) => {
                self.active_stream_content.clear();
                self.active_stream_thought.clear();
                self.stream_rx = Some(rx_arc);
            }
            Message::StreamChunkReceived(chunk) => match chunk {
                StreamChunk::Thought(t) => {
                    self.active_stream_thought.push_str(&t);
                }
                StreamChunk::Token(token) => {
                    self.active_stream_content.push_str(&token);
                }
                StreamChunk::Metrics(tps) => {
                    self.live_tok_per_sec = Some(tps);
                }
                StreamChunk::Error(err) => {
                    self.active_stream_content
                        .push_str(&format!("\n\nHata: {err}"));
                    self.is_loading = false;
                    self.stream_rx = None;
                    self.cancel_flag = None;
                    let thought = if self.active_stream_thought.trim().is_empty() {
                        None
                    } else {
                        Some(self.active_stream_thought.clone())
                    };
                    if !self.active_stream_content.trim().is_empty() {
                        self.history.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content: self.active_stream_content.clone(),
                            thought,
                        });
                    }
                    self.active_stream_content.clear();
                    self.active_stream_thought.clear();
                }
                StreamChunk::Done => {
                    self.is_loading = false;
                    self.stream_rx = None;
                    self.cancel_flag = None;
                    let thought = if self.active_stream_thought.trim().is_empty() {
                        None
                    } else {
                        Some(self.active_stream_thought.clone())
                    };
                    if !self.active_stream_content.trim().is_empty() {
                        self.history.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content: self.active_stream_content.clone(),
                            thought,
                        });
                    }
                    self.active_stream_content.clear();
                    self.active_stream_thought.clear();
                }
            },
            Message::CopyCode(code, idx) => {
                self.copied_index = Some(idx);
                std::thread::spawn(move || {
                    AiEngine::copy_to_clipboard(&code);
                });
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    },
                    |_| Message::ResetCopyStatus,
                );
            }
            Message::ResetCopyStatus => {
                self.copied_index = None;
            }
            Message::CursorTick => {
                self.cursor_blink = !self.cursor_blink;
                if let Ok(app_cfg) =
                    ConfigManager::<AppearanceConfig>::load_or_default("appearance")
                {
                    let new_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    if new_patina != self.active_patina {
                        self.active_patina = new_patina;
                    }
                }
                if let Ok(ai_cfg_mgr) = ConfigManager::<AiSettingsConfig>::load_or_default("ai") {
                    let current_ai = ai_cfg_mgr.get().clone();
                    if current_ai != self.ai_config {
                        self.ai_config = current_ai;
                        let cfg = self.ai_config.clone();
                        return Task::perform(
                            async move { AiEngine::auto_detect_with_config(Some(&cfg)).await },
                            Message::BackendDetected,
                        );
                    }
                }
            }
            Message::CloseRequested => {
                std::process::exit(0);
            }
            Message::EventTriggered(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            })) => {
                if self.show_model_picker {
                    self.show_model_picker = false;
                } else if self.is_loading {
                    return Task::done(Message::StopStreaming);
                } else {
                    std::process::exit(0);
                }
            }
            Message::EventTriggered(_) => {}
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        // 1. Minimal M3 Header Bar
        let header_row = row![
            row![
                text(Icon::SPARKLES)
                    .font(LUCIDE_FONT)
                    .size(16.0)
                    .color(palette.primary.to_iced()),
                text("Corten AI")
                    .font(NOTO_SANS_SEMIBOLD)
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
            horizontal_space(),
            Self::header_icon_btn(
                Icon::APP_WINDOW,
                if self.layout_mode == DeckLayoutMode::SideDeck {
                    "Yüzen"
                } else {
                    "Kenar"
                },
                palette,
                false,
                Message::ToggleDeckMode,
            ),
            Self::header_icon_btn(Icon::TRASH, "Temizle", palette, false, Message::ClearChat),
            Self::header_icon_btn(Icon::X, "", palette, true, Message::CloseRequested),
        ]
        .spacing(Spacing::XXS)
        .align_y(Alignment::Center);

        // 2. Dynamic Model Picker + Mode Switch Bar (replaces hardcoded 1.5B/3B/7B tiers)
        let active_model_label = self
            .backend
            .active_model_name()
            .map(truncate_model_name)
            .unwrap_or_else(|| "Model bağlı değil".to_string());
        let has_models = !self.backend.models().is_empty();

        let model_picker_btn = button(
            row![
                text(Icon::LAYERS)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(if has_models {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                text(active_model_label)
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
                text(if self.show_model_picker {
                    Icon::ARROW_UP
                } else {
                    Icon::ARROW_DOWN
                })
                .font(LUCIDE_FONT)
                .size(11.0)
                .color(palette.text_muted.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered if has_models => palette.surface_hover.to_iced(),
                _ => palette.surface.to_iced(),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            }
        })
        .on_press_maybe(has_models.then_some(Message::ToggleModelPicker));

        let chat_active = self.chat_mode == ChatMode::Chat;
        let agent_active = self.chat_mode == ChatMode::Agent;
        let agent_supported = self.backend.supports_agent_mode();

        let mode_switch = container(
            row![
                Self::mode_pill(
                    "Sohbet",
                    chat_active,
                    true,
                    palette,
                    Message::SetChatMode(ChatMode::Chat)
                ),
                Self::mode_pill(
                    "Ajan",
                    agent_active,
                    agent_supported,
                    palette,
                    Message::SetChatMode(ChatMode::Agent),
                ),
            ]
            .spacing(2),
        )
        .padding(2)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(palette.surface.to_iced())),
            border: iced::Border {
                radius: iced::border::Radius::from(Radius::FULL),
                ..Default::default()
            },
            ..Default::default()
        });

        let refresh_btn = button(
            text(Icon::REDO)
                .font(LUCIDE_FONT)
                .size(12.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([6, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.surface_hover.to_iced(),
                _ => palette.surface.to_iced(),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::RefreshBackend);

        let control_row = row![
            model_picker_btn,
            horizontal_space(),
            mode_switch,
            refresh_btn
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center);

        // Optional Dynamic Model Picker Dropdown
        let model_dropdown: Option<Element<'_, Message>> = if self.show_model_picker {
            let active_id = self.backend.active_model_name().unwrap_or_default();
            let mut list = column![].spacing(4);
            for entry in self.backend.models() {
                let is_selected = entry.id == active_id;
                let id_clone = entry.id.clone();
                let mut item_row = row![
                    text(if is_selected { Icon::CHECK } else { Icon::CPU })
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(if is_selected {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(&entry.label)
                        .size(12.0)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                ]
                .spacing(8)
                .align_y(Alignment::Center);

                if !entry.size_label.is_empty() {
                    item_row = item_row.push(
                        text(&entry.size_label)
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    );
                }

                list = list.push(
                    button(item_row)
                        .padding([7, 10])
                        .width(Length::Fill)
                        .style(move |_, status| {
                            let bg = if is_selected {
                                iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.18,
                                )
                            } else {
                                match status {
                                    button::Status::Hovered => palette.surface_hover.to_iced(),
                                    _ => iced::Color::TRANSPARENT,
                                }
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::SelectModel(id_clone)),
                );
            }

            Some(
                container(scrollable(list).height(Length::Shrink))
                    .max_height(220.0)
                    .padding(Spacing::XS)
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.surface.to_iced())),
                        border: iced::Border {
                            color: iced::Color::from_rgba(
                                palette.border.r,
                                palette.border.g,
                                palette.border.b,
                                0.55,
                            ),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    })
                    .into(),
            )
        } else {
            None
        };

        // 3. Conversation Feed
        let mut chat_column = column![].spacing(Spacing::SM).padding(Spacing::SM);

        if let Some(notice) = &self.status_notice {
            chat_column = chat_column.push(
                container(
                    text(notice)
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([6, 10])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(palette.surface.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        if self.history.is_empty()
            && self.active_stream_content.is_empty()
            && self.active_stream_thought.is_empty()
        {
            if matches!(self.backend, BackendType::Offline) {
                let launch_label = if self.is_launching_daemon {
                    "Başlatılıyor..."
                } else {
                    "AURA-CORE Başlat (UDS)"
                };
                let offline_card = container(
                    column![
                        row![
                            text(Icon::CPU)
                                .font(LUCIDE_FONT)
                                .size(18.0)
                                .color(palette.primary.to_iced()),
                            text("Yerel AI sunucusu bağlı değil")
                                .font(NOTO_SANS_SEMIBOLD)
                                .size(Typography::BODY)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_y(Alignment::Center),
                        text("Corten AI herhangi bir boyuttaki yerel modeli (3B, 14B, 30B+) AURA-CORE (UDS/TCP), Ollama veya llama-server üzerinden çalıştırır.")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                        vertical_space().height(Spacing::XXS),
                        row![
                            button(
                                row![
                                    text(Icon::PLAY)
                                        .font(LUCIDE_FONT)
                                        .size(12.0)
                                        .color(palette.text_primary.to_iced()),
                                    text(launch_label)
                                        .size(12.0)
                                        .color(palette.text_primary.to_iced()),
                                ]
                                .spacing(6)
                                .align_y(Alignment::Center)
                            )
                            .padding([8, 14])
                            .style(move |_, status| {
                                let bg = match status {
                                    button::Status::Hovered => palette.primary_hover.to_iced(),
                                    _ => palette.primary.to_iced(),
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(Radius::FULL),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press_maybe((!self.is_launching_daemon).then_some(Message::LaunchAuraDaemon)),
                            button(
                                text("Yeniden Tara")
                                    .size(12.0)
                                    .color(palette.text_primary.to_iced())
                            )
                            .padding([8, 14])
                            .style(move |_, status| {
                                let bg = match status {
                                    button::Status::Hovered => palette.surface_hover.to_iced(),
                                    _ => palette.surface.to_iced(),
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(Radius::FULL),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::RefreshBackend),
                        ]
                        .spacing(Spacing::XS),
                        text("Otomatik algılama sırası: aura.sock (UDS) · port 11435 · Ollama (11434) · llama-server (8080)")
                            .size(10.5)
                            .color(palette.text_disabled.to_iced()),
                    ]
                    .spacing(Spacing::XS),
                )
                .padding(Spacing::MD)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(palette.surface.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::MD),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                chat_column = chat_column.push(offline_card);
            } else {
                let welcome_card = container(
                    column![
                        text("Nasıl yardımcı olabilirim?")
                            .font(NOTO_SANS_SEMIBOLD)
                            .size(Typography::BODY)
                            .color(palette.text_primary.to_iced()),
                        text("Yukarıdaki menüden dilediğiniz modeli seçebilir, AURA-CORE bağlıyken Ajan moduna geçebilirsiniz.")
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                        vertical_space().height(Spacing::XXS),
                        row![
                            Self::prompt_chip("Rust borrow checker özetle", palette),
                            Self::prompt_chip("Linux disk kullanımını nasıl kontrol ederim?", palette),
                        ]
                        .spacing(Spacing::XS),
                    ]
                    .spacing(Spacing::XS),
                )
                .padding(Spacing::MD)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(palette.surface.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::MD),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                chat_column = chat_column.push(welcome_card);
            }
        }

        let mut code_block_idx = 0;

        for (msg_idx, msg) in self.history.iter().enumerate() {
            match msg.role {
                MessageRole::User => {
                    let user_bubble = container(
                        text(&msg.content)
                            .size(Typography::BODY_SM)
                            .color(palette.text_primary.to_iced()),
                    )
                    .padding([9, 14])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.20,
                        ))),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::MD),
                            ..Default::default()
                        },
                        ..Default::default()
                    });

                    chat_column = chat_column.push(
                        row![horizontal_space(), user_bubble]
                            .width(Length::Fill)
                            .align_y(Alignment::Center),
                    );
                }
                MessageRole::Assistant | MessageRole::System => {
                    let mut assistant_box = column![].spacing(Spacing::XS).width(Length::Fill);

                    if let Some(ref thought) = msg.thought {
                        let is_open = self.expanded_thoughts.contains(&msg_idx);
                        let toggle_btn = button(
                            text(if is_open {
                                "Düşünce Süreci (Gizle)"
                            } else {
                                "Düşünce Süreci (Göster)"
                            })
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        )
                        .padding([4, 8])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => palette.surface_hover.to_iced(),
                                _ => palette.background.to_iced(),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::ToggleThought(msg_idx));

                        let thought_box = if is_open {
                            column![
                                toggle_btn,
                                container(
                                    text(thought).size(11.0).color(palette.text_muted.to_iced())
                                )
                                .padding([8, 10])
                                .width(Length::Fill)
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        palette.background.to_iced(),
                                    )),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(Radius::SM),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                            ]
                            .spacing(4)
                        } else {
                            column![toggle_btn]
                        };
                        assistant_box = assistant_box.push(thought_box);
                    }

                    let rendered = Self::render_rich_message(
                        &msg.content,
                        palette,
                        &mut code_block_idx,
                        self.copied_index,
                    );
                    assistant_box = assistant_box.push(rendered);

                    let wrapped_card = container(assistant_box)
                        .padding([10, 12])
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.surface.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::MD),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                    chat_column = chat_column.push(wrapped_card);
                }
            }
        }

        // Active streaming thought
        if !self.active_stream_thought.is_empty() {
            let is_open = self.active_thought_expanded;
            let toggle_btn = button(
                text(if is_open {
                    "Düşünce Akışı (Gizle)"
                } else {
                    "Düşünce Akışı (Göster)"
                })
                .size(11.0)
                .color(palette.warning.to_iced()),
            )
            .padding([4, 8])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => palette.surface_hover.to_iced(),
                    _ => palette.surface.to_iced(),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleActiveThought);

            let thought_box = if is_open {
                column![
                    toggle_btn,
                    container(
                        text(&self.active_stream_thought)
                            .size(11.0)
                            .color(palette.text_muted.to_iced())
                    )
                    .padding([8, 10])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.surface.to_iced())),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::SM),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                ]
                .spacing(4)
            } else {
                column![toggle_btn]
            };
            chat_column = chat_column.push(thought_box);
        }

        // Active streaming content
        if !self.active_stream_content.is_empty()
            || (self.is_loading && self.active_stream_thought.is_empty())
        {
            let mut stream_text = self.active_stream_content.clone();
            if self.is_loading {
                stream_text.push_str(if self.cursor_blink { " |" } else { "  " });
            }
            let rendered = if !stream_text.contains("```") {
                container(
                    text(stream_text)
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                )
                .padding([10, 12])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(palette.surface.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::MD),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            } else {
                Self::render_rich_message(
                    &stream_text,
                    palette,
                    &mut code_block_idx,
                    self.copied_index,
                )
            };
            chat_column = chat_column.push(rendered);
        }

        let chat_box = container(scrollable(chat_column).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.22,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::MD),
                    ..Default::default()
                },
                ..Default::default()
            });

        // 4. Input Bar
        let input_elem = text_input(
            if self.chat_mode == ChatMode::Agent {
                "Ajan görevi yazın..."
            } else {
                "Bir soru sorun..."
            },
            &self.query_input,
        )
        .padding([Spacing::SM, Spacing::MD])
        .on_input(Message::QueryInputChanged)
        .on_submit(Message::SubmitQuery)
        .width(Length::Fill)
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
        });

        let send_or_stop_btn = if self.is_loading {
            button(
                row![
                    text(Icon::SQUARE)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced()),
                    text("Durdur")
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM, Spacing::MD])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(0.90, 0.28, 0.22, 0.95),
                    _ => palette.danger.to_iced(),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::StopStreaming)
        } else {
            button(
                text("Gönder")
                    .size(Typography::BODY_SM)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([Spacing::SM, Spacing::LG])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => palette.primary_hover.to_iced(),
                    _ => palette.primary.to_iced(),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::SubmitQuery)
        };

        let input_container = container(
            row![input_elem, send_or_stop_btn]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
        )
        .padding([2, 4])
        .style(move |_| input_style(palette));

        // 5. Clean Single-Line Status Footer (metrics live here, never inside chat text)
        let right_footer = match self.live_tok_per_sec {
            Some(tps) if tps > 0.0 => format!("{tps:.1} tok/sn"),
            _ => "Enter: Gönder · Esc: Durdur / Kapat".to_string(),
        };

        let micro_footer = row![
            text(self.backend.status_label())
                .size(11.0)
                .color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(right_footer)
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        ]
        .align_y(Alignment::Center);

        // 6. Assemble Card (M3 tonal surface, zero neon wireframe border)
        let mut main_card_column = column![header_row, control_row].spacing(Spacing::SM);
        if let Some(dropdown) = model_dropdown {
            main_card_column = main_card_column.push(dropdown);
        }
        main_card_column = main_card_column
            .push(chat_box)
            .push(input_container)
            .push(micro_footer);

        let is_floating = self.layout_mode == DeckLayoutMode::FloatingHub;
        let bg_color = iced::Color::from_rgba(
            palette.background.r,
            palette.background.g,
            palette.background.b,
            0.96,
        );
        let subtle_outline =
            iced::Color::from_rgba(palette.border.r, palette.border.g, palette.border.b, 0.40);

        let main_card = if is_floating {
            container(main_card_column)
                .width(Length::Fixed(760.0))
                .height(Length::Fixed(640.0))
                .padding(Spacing::MD)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    border: iced::Border {
                        color: subtle_outline,
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                        offset: iced::Vector::new(0.0, 8.0),
                        blur_radius: 28.0,
                    },
                    ..Default::default()
                })
        } else {
            container(main_card_column)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(Spacing::MD)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    border: iced::Border {
                        color: subtle_outline,
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                        offset: iced::Vector::new(-4.0, 0.0),
                        blur_radius: 24.0,
                    },
                    ..Default::default()
                })
        };

        if is_floating {
            container(main_card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            container(main_card)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen().map(Message::EventTriggered);
        let tick =
            iced::time::every(std::time::Duration::from_millis(1000)).map(|_| Message::CursorTick);
        let mut subs = vec![events, tick];

        if self.is_loading {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(350))
                    .map(|_| Message::CursorTick),
            );
        }

        if let Some(rx_arc) = &self.stream_rx {
            let rx_clone = Arc::clone(rx_arc);
            let stream = futures::stream::unfold(rx_clone, |rx| async move {
                let mut lock = rx.lock().await;
                let chunk = lock.recv().await?;
                drop(lock);
                Some((Message::StreamChunkReceived(chunk), rx))
            });
            let stream_sub = Subscription::run_with_id("corten-ai-token-stream", stream);
            subs.push(stream_sub);
        }

        Subscription::batch(subs)
    }
}

fn truncate_model_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() <= 28 {
        trimmed.to_string()
    } else {
        let short: String = trimmed.chars().take(26).collect();
        format!("{short}...")
    }
}

enum ResponseSegment {
    Text(String),
    Code { lang: String, code: String },
}

fn parse_response(raw: &str) -> Vec<ResponseSegment> {
    let mut segments = Vec::new();
    let parts: Vec<&str> = raw.split("```").collect();
    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 0 {
            if !part.trim().is_empty() {
                segments.push(ResponseSegment::Text(part.to_string()));
            }
        } else {
            let (lang, code_content) = match part.split_once('\n') {
                Some((first, rest)) => {
                    let trimmed = first.trim();
                    if !trimmed.is_empty() && !trimmed.contains(' ') {
                        (trimmed.to_uppercase(), rest.trim_end())
                    } else {
                        ("KOD".to_string(), part.trim())
                    }
                }
                None => ("KOD".to_string(), part.trim()),
            };
            segments.push(ResponseSegment::Code {
                lang: if lang.is_empty() { "KOD".into() } else { lang },
                code: code_content.to_string(),
            });
        }
    }
    if segments.is_empty() {
        segments.push(ResponseSegment::Text(raw.to_string()));
    }
    segments
}

impl CortenAiApp {
    fn header_icon_btn<'a>(
        icon: char,
        label: &'static str,
        palette: Palette,
        is_danger: bool,
        msg: Message,
    ) -> Element<'a, Message> {
        let mut content = row![
            text(icon)
                .font(LUCIDE_FONT)
                .size(12.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        if !label.is_empty() {
            content = content.push(text(label).size(11.5).color(palette.text_muted.to_iced()));
        }

        button(content)
            .padding([5, 9])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered if is_danger => {
                        iced::Color::from_rgba(0.85, 0.25, 0.20, 0.35)
                    }
                    button::Status::Hovered => palette.surface_hover.to_iced(),
                    _ => iced::Color::TRANSPARENT,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(msg)
            .into()
    }

    fn mode_pill<'a>(
        label: &'static str,
        active: bool,
        enabled: bool,
        palette: Palette,
        msg: Message,
    ) -> Element<'a, Message> {
        let text_color = if active {
            palette.text_primary.to_iced()
        } else if enabled {
            palette.text_muted.to_iced()
        } else {
            palette.text_disabled.to_iced()
        };

        button(text(label).size(11.5).color(text_color))
            .padding([4, 10])
            .style(move |_, status| {
                let bg = if active {
                    iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.25,
                    )
                } else {
                    match status {
                        button::Status::Hovered if enabled => palette.surface_hover.to_iced(),
                        _ => iced::Color::TRANSPARENT,
                    }
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(msg)
            .into()
    }

    fn prompt_chip<'a>(prompt: &'static str, palette: Palette) -> Element<'a, Message> {
        button(
            text(prompt)
                .size(11.0)
                .color(palette.text_primary.to_iced()),
        )
        .padding([5, 10])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.surface_hover.to_iced(),
                _ => palette.background.to_iced(),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::QuickAction(prompt.to_string()))
        .into()
    }

    fn render_rich_message<'a>(
        raw: &str,
        palette: Palette,
        code_idx_counter: &mut usize,
        copied_idx: Option<usize>,
    ) -> Element<'a, Message> {
        let mut msg_items = column![].spacing(Spacing::SM).width(Length::Fill);
        let segments = parse_response(raw);

        for seg in segments {
            match seg {
                ResponseSegment::Text(t) => {
                    msg_items = msg_items.push(
                        text(t)
                            .size(Typography::BODY_SM)
                            .color(palette.text_primary.to_iced()),
                    );
                }
                ResponseSegment::Code { lang, code } => {
                    let idx = *code_idx_counter;
                    *code_idx_counter += 1;
                    let is_copied = copied_idx == Some(idx);
                    let code_clone = code.clone();

                    let header = row![
                        text(lang).size(10.5).color(palette.primary.to_iced()),
                        horizontal_space(),
                        button(
                            row![
                                text(if is_copied { Icon::CHECK } else { Icon::COPY })
                                    .font(LUCIDE_FONT)
                                    .size(11.0)
                                    .color(if is_copied {
                                        palette.success.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                                text(if is_copied { "Kopyalandı" } else { "Kopyala" })
                                    .size(10.5)
                                    .color(if is_copied {
                                        palette.success.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                        )
                        .padding([2, 6])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => palette.surface_hover.to_iced(),
                                _ => iced::Color::TRANSPARENT,
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::CopyCode(code_clone, idx)),
                    ]
                    .align_y(Alignment::Center);

                    let code_card = container(
                        column![
                            header,
                            text(code)
                                .size(12.0)
                                .font(iced::Font::MONOSPACE)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(Spacing::XS),
                    )
                    .padding([8, 10])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.background.to_iced())),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::SM),
                            ..Default::default()
                        },
                        ..Default::default()
                    });

                    msg_items = msg_items.push(code_card);
                }
            }
        }

        msg_items.into()
    }
}
