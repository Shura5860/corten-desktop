use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::info;

use corten_config::{AiSettingsConfig, AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::input_style;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES,
};
use iced::keyboard::key::Named;
use iced::keyboard::Key;
use iced::widget::{
    button, column, container, horizontal_space, row, scrollable, text, text_input, vertical_space,
};
use iced::{event, Alignment, Element, Event, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::Mutex;

mod actions;
mod aura;
mod engine;

use engine::{AiEngine, BackendType, ChatMessage, MessageRole, StreamChunk};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                println!("Corten AI (AURA-CORE v0.2.0) — Saf Rust Yerel Yapay Zekâ & Masaüstü Ajanı");
                println!("Kullanım:");
                println!("  corten-ai                        Masaüstü HUD arayüzünü başlatır (Wayland)");
                println!("  corten-ai --ask <SORGU>          Terminal üzerinden doğrudan AURA-CORE sorgusu çalıştırır");
                println!("  corten-ai --prompt <SORGU>       HUD arayüzünü belirtilen sorguyla açar");
                println!("  corten-ai --audit <DOSYA>        Belirtilen dosyayı güvenlik denetimiyle HUD'da açar");
                println!("  corten-ai --daemon               Arka plan indeksleme ve bellek servisini çalıştırır");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("corten-ai 0.2.0 (Pure Rust AURA-CORE — Executive Side-Deck)");
                return Ok(());
            }
            "--ask" | "-q" if args.len() > 2 => {
                let query = args[2..].join(" ");
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(async move {
                    let backend = AiEngine::auto_detect().await;
                    let (tx, mut rx) = unbounded_channel();
                    AiEngine::stream_query(backend, Vec::new(), query, None, tx).await;
                    while let Some(chunk) = rx.recv().await {
                        match chunk {
                            StreamChunk::Token(tok) => print!("{tok}"),
                            StreamChunk::Thought(_) => {}
                            StreamChunk::ActionExecuted(act) => println!("\n{act}"),
                            StreamChunk::Error(err) => eprintln!("Hata: {err}"),
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
                info!("Starting Corten AURA-CORE background daemon...");
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(async move {
                    let backend = AiEngine::auto_detect().await;
                    info!("AURA-CORE daemon active with backend: {:?}", backend);
                    std::future::pending::<()>().await;
                });
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

    info!("Starting Corten AI Assistant Executive Side-Deck (corten-ai v0.2.0)...");

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
    AnalyzeScreenRequested,
    RefreshBackend,
    BackendDetected(BackendType),
    SelectModelTier(String),
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
    history: Vec<ChatMessage>,
    active_stream_content: String,
    active_stream_thought: String,
    stream_rx: Option<Arc<Mutex<UnboundedReceiver<StreamChunk>>>>,
    query_input: String,
    is_loading: bool,
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
            history: Vec::new(),
            active_stream_content: String::new(),
            active_stream_thought: String::new(),
            stream_rx: None,
            query_input: initial_query.clone().unwrap_or_default(),
            is_loading: initial_query.is_some(),
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
                if let Some(q) = opt_q {
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
                    self.is_loading = false;
                    self.stream_rx = None;
                    let thought = if self.active_stream_thought.trim().is_empty() {
                        None
                    } else {
                        Some(self.active_stream_thought.clone())
                    };
                    let mut content = self.active_stream_content.clone();
                    if !content.trim().is_empty() {
                        content.push_str("\n\n*(Üretim kullanıcı tarafından durduruldu)*");
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
                let cfg = self.ai_config.clone();
                return Task::perform(
                    async move { AiEngine::auto_detect_with_config(Some(&cfg)).await },
                    Message::BackendDetected,
                );
            }
            Message::BackendDetected(b) => {
                self.backend = b;
            }
            Message::SelectModelTier(new_tier) => {
                if self.ai_config.model_tier != new_tier {
                    self.ai_config.model_tier = new_tier;
                    if let Ok(mut mgr) = ConfigManager::<AiSettingsConfig>::load_or_default("ai") {
                        let _ = mgr.update(self.ai_config.clone());
                    }
                    let cfg = self.ai_config.clone();
                    return Task::perform(
                        async move { AiEngine::auto_detect_with_config(Some(&cfg)).await },
                        Message::BackendDetected,
                    );
                }
            }
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
                            780, 640,
                        )))),
                    ]),
                };
            }
            Message::ClearChat => {
                self.history.clear();
                self.active_stream_content.clear();
                self.active_stream_thought.clear();
                self.expanded_thoughts.clear();
                self.is_loading = false;
            }
            Message::SubmitQuery => {
                let q = self.query_input.trim().to_string();
                if !q.is_empty() && !self.is_loading {
                    self.is_loading = true;
                    self.active_stream_content.clear();
                    self.active_stream_thought.clear();
                    self.query_input.clear();

                    self.history.push(ChatMessage {
                        role: MessageRole::User,
                        content: q.clone(),
                        thought: None,
                    });

                    let backend = self.backend.clone();
                    let history = self.history.clone();

                    return Task::perform(
                        async move {
                            let (tx, rx) = unbounded_channel();
                            tokio::spawn(async move {
                                AiEngine::stream_query(backend, history, q, None, tx).await;
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
            Message::AnalyzeScreenRequested => {
                if !self.is_loading {
                    self.is_loading = true;
                    self.active_stream_content =
                        "📸 Ekran görüntüsü yakalandı. Görsel analiz yapılıyor...".into();
                    self.active_stream_thought.clear();

                    let query = "Şu an ekranda ne var? Açık pencereleri, içeriği ve önemli ayrıntıları analiz et.".to_string();
                    self.history.push(ChatMessage {
                        role: MessageRole::User,
                        content: "📸 [Ekran Görsel Analiz Talebi]".into(),
                        thought: None,
                    });

                    let backend = self.backend.clone();
                    let history = self.history.clone();

                    return Task::perform(
                        async move {
                            let screen_base64 = AiEngine::capture_screen_base64();
                            let (tx, rx) = unbounded_channel();
                            tokio::spawn(async move {
                                AiEngine::stream_query(backend, history, query, screen_base64, tx)
                                    .await;
                            });
                            Arc::new(Mutex::new(rx))
                        },
                        Message::StreamStarted,
                    );
                }
            }
            Message::StreamStarted(rx_arc) => {
                self.active_stream_content.clear();
                self.active_stream_thought.clear();
                self.stream_rx = Some(rx_arc);
            }
            Message::StreamChunkReceived(chunk) => {
                match chunk {
                    StreamChunk::Thought(t) => {
                        self.active_stream_thought.push_str(&t);
                    }
                    StreamChunk::Token(token) => {
                        self.active_stream_content.push_str(&token);
                    }
                    StreamChunk::ActionExecuted(feedback) => {
                        self.active_stream_content.push_str(&format!("\n\n{}", feedback));
                    }
                    StreamChunk::Error(err) => {
                        self.active_stream_content.push_str(&format!("\n\n⚠️ Hata: {}", err));
                        self.is_loading = false;
                        self.stream_rx = None;
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
                }
            }
            Message::CopyCode(code, idx) => {
                self.copied_index = Some(idx);
                let code_to_copy = code;
                std::thread::spawn(move || {
                    use std::io::Write;
                    if let Ok(mut child) = std::process::Command::new("wl-copy")
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                    {
                        if let Some(mut stdin) = child.stdin.take() {
                            let _ = stdin.write_all(code_to_copy.as_bytes());
                        }
                    }
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
                if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
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
                if self.is_loading {
                    return Task::done(Message::StopStreaming);
                }
                std::process::exit(0);
            }
            Message::EventTriggered(_) => {}
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        // 1. Header: Brand, Mode Switcher, Clear & Close Buttons
        let mode_label = if self.layout_mode == DeckLayoutMode::SideDeck {
            "⧉ Yüzen Mod"
        } else {
            "⇲ Kenar Deck"
        };

        let header_row = row![
            row![
                text("⬡ CORTEN AURA")
                    .size(Typography::TITLE_SM)
                    .color(palette.primary.to_iced()),
                container(
                    text("v0.2.0")
                        .size(10.0)
                        .color(palette.text_muted.to_iced())
                )
                .padding([2, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
            horizontal_space(),
            button(
                text(mode_label)
                    .size(11.0)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([4, 8])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleDeckMode),
            button(
                text("🧹 Temizle")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([4, 8])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ClearChat),
            button(
                text("✕ ESC")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([4, 8])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(0.85, 0.25, 0.2, 0.35),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::CloseRequested),
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center);

        // 2. Bento Telemetry Box: Status & Model Tier Pills
        let (status_label, engine_detail) = match &self.backend {
            BackendType::NativeGguf { model_name, .. } => (
                format!("Saf Rust GGUF ({})", model_name),
                "Yerel Bellek • Optimize",
            ),
            BackendType::Ollama {
                model,
                tier_label,
                has_vision,
                ..
            } => (
                format!("Ollama: {} [{}]", model, tier_label),
                if *has_vision {
                    "Aktif • 👁 Görsel Destekli"
                } else {
                    "Aktif Motor • 11434"
                },
            ),
            BackendType::LlamaCpp { model, .. } => (
                format!("llama-server: {}", model),
                "Aktif Motor • 8080",
            ),
            BackendType::AuraCore { .. } => (
                "AURA-CORE (8000)".to_string(),
                "Yüksek Başarım Motoru",
            ),
            BackendType::Offline => (
                "Çevrimdışı (Masaüstü Eylem Modu)".to_string(),
                "Yerel Araçlar • Çevrimdışı",
            ),
        };

        let is_eco = self.ai_config.model_tier.to_lowercase() == "eco";
        let is_balanced = self.ai_config.model_tier.to_lowercase() == "balanced";
        let is_pro = self.ai_config.model_tier.to_lowercase() == "pro";

        let telemetry_bento = container(
            column![
                row![
                    button(
                        row![
                            text(Icon::CPU).font(LUCIDE_FONT).size(13.0).color(palette.primary.to_iced()),
                            text(if self.is_loading {
                                "Düşünüyor / Üretiyor...".to_string()
                            } else {
                                status_label.clone()
                            })
                            .size(11.5)
                            .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center)
                    )
                    .padding(0)
                    .style(|_, _| button::Style::default())
                    .on_press(Message::RefreshBackend),
                    horizontal_space(),
                    text(engine_detail)
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                ]
                .align_y(Alignment::Center),
                row![
                    text("Kademe:")
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                    Self::tier_pill_button("Eco (1.5B)", "eco", is_eco, palette),
                    Self::tier_pill_button("Dengeli (3B)", "balanced", is_balanced, palette),
                    Self::tier_pill_button("Pro (7B)", "pro", is_pro, palette),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
            ]
            .spacing(Spacing::XS),
        )
        .padding([8, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.025))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::MD),
            },
            ..Default::default()
        });

        // 3. Bento Quick Action Strip
        let chips_row = row![
            Self::chip_button("📸 Ekran Analizi", palette, Message::AnalyzeScreenRequested),
            Self::chip_button(
                "⚡ Sistem Durumu",
                palette,
                Message::QuickAction("sistem durumu ve donanım kullanımını özetle".into())
            ),
            Self::chip_button(
                "🤖 ReAct Ajan",
                palette,
                Message::QuickAction("react: açık pencereleri ve masaüstü durumunu listele".into())
            ),
            Self::chip_button(
                "🔍 Vektör Ara",
                palette,
                Message::QuickAction("vektör ara: wayland".into())
            ),
            Self::chip_button("🧠 Hafıza", palette, Message::QuickAction("hafıza".into())),
        ]
        .spacing(Spacing::XS);

        // 4. Conversation Feed
        let mut chat_column = column![].spacing(Spacing::MD).padding(Spacing::XS);

        if self.history.is_empty()
            && self.active_stream_content.is_empty()
            && self.active_stream_thought.is_empty()
        {
            let welcome_bento = container(
                column![
                    row![
                        text("⚡")
                            .size(Typography::TITLE)
                            .color(palette.primary.to_iced()),
                        column![
                            text("Corten Executive Copilot")
                                .size(Typography::BODY)
                                .color(palette.text_primary.to_iced()),
                            text("Saf Rust mimarili, yerel ve tam gizlilik odaklı masaüstü asistanı.")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2),
                    ]
                    .spacing(Spacing::SM)
                    .align_y(Alignment::Center),
                    vertical_space().height(Spacing::XXS),
                    row![
                        Self::feature_mini_card(
                            "🖥️ Masaüstü Yönetimi",
                            "Ses, parlaklık, pencere ve çalışma alanlarını doğrudan yönetin.",
                            palette
                        ),
                        Self::feature_mini_card(
                            "📸 Multimodal Görsel",
                            "Ekranda ne olduğunu tek tıkla inceletip teşhis alın.",
                            palette
                        ),
                    ]
                    .spacing(Spacing::XS),
                    row![
                        Self::feature_mini_card(
                            "🛠️ Sistem Teşhisi",
                            "Donanım, bellek ve disk durumunu anında raporlayın.",
                            palette
                        ),
                        Self::feature_mini_card(
                            "🔒 %100 Yerel Mimari",
                            "Verileriniz cihazınızda kalır, telemetri sızdırılmaz.",
                            palette
                        ),
                    ]
                    .spacing(Spacing::XS),
                    vertical_space().height(Spacing::XXS),
                    text("Hızlı İpuçları:")
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                    row![
                        Self::prompt_chip("\"Gece ışığını aç ve sesi %50 yap\"", palette),
                        Self::prompt_chip("\"En çok RAM kullanan uygulamalar\"", palette),
                    ]
                    .spacing(Spacing::XS),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::MD)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::LG),
                },
                ..Default::default()
            });

            chat_column = chat_column.push(welcome_bento);
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
                    .padding([8, 14])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.18,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.45,
                            ),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::MD),
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
                                "▾ 💭 Düşünce Süreci (Gizle)"
                            } else {
                                "▸ 💭 Düşünce Süreci (Aç)"
                            })
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        )
                        .padding([3, 7])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                                }
                                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::ToggleThought(msg_idx));

                        let thought_box = if is_open {
                            column![
                                toggle_btn,
                                container(
                                    text(thought)
                                        .size(11.0)
                                        .color(palette.text_muted.to_iced())
                                )
                                .padding([8, 10])
                                .width(Length::Fill)
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                                    )),
                                    border: iced::Border {
                                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(Radius::SM),
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
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                1.0, 1.0, 1.0, 0.02,
                            ))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::MD),
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
                    "▾ 💭 Düşünce Akışı (Gizle)"
                } else {
                    "▸ 💭 Düşünce Akışı (Göster)"
                })
                .size(11.0)
                .color(palette.warning.to_iced()),
            )
            .padding([3, 7])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
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
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            0.0, 0.0, 0.0, 0.35,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
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
                stream_text.push_str(if self.cursor_blink { " ▋" } else { "  " });
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
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.02,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::MD),
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
                    0.0, 0.0, 0.0, 0.35,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            });

        // 5. Input Capsule & Control Deck
        let screen_btn = button(
            text("📸")
                .size(Typography::BODY)
                .color(palette.primary.to_iced()),
        )
        .padding([Spacing::SM, Spacing::MD])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
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
        .on_press(Message::AnalyzeScreenRequested);

        let input_elem = text_input(
            "Corten AURA'ya sor, eylem iste veya kod analiz et...",
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
                    text("⏹ Durdur")
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                    container(text("Esc").size(9.0).color(palette.text_primary.to_iced()))
                        .padding([1, 4])
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                0.0, 0.0, 0.0, 0.3,
                            ))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::SM, Spacing::MD])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(0.9, 0.28, 0.22, 0.95),
                    _ => iced::Color::from_rgba(0.8, 0.22, 0.18, 0.85),
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
                text("Gönder ↵")
                    .size(Typography::BODY_SM)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([Spacing::SM, Spacing::LG])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.primary.to_iced())),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::SubmitQuery)
        };

        let input_container = container(
            row![screen_btn, input_elem, send_or_stop_btn]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
        )
        .style(move |_| input_style(palette));

        let micro_footer = row![
            text("Enter = Gönder • Esc = Kapat / Durdur")
                .size(10.5)
                .color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(format!(
                "Tier: {} • {}",
                self.ai_config.model_tier.to_uppercase(),
                status_label
            ))
            .size(10.5)
            .color(palette.text_muted.to_iced()),
        ]
        .align_y(Alignment::Center);

        // 6. Assemble Main Bento Card
        let main_card_column = column![
            header_row,
            telemetry_bento,
            chips_row,
            chat_box,
            input_container,
            micro_footer,
        ]
        .spacing(Spacing::SM);

        let is_floating = self.layout_mode == DeckLayoutMode::FloatingHub;

        let main_card = if is_floating {
            container(main_card_column)
                .width(Length::Fixed(780.0))
                .height(Length::Fixed(640.0))
                .padding(Spacing::LG)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.07, 0.08, 0.10, 0.96,
                    ))),
                    border: iced::Border {
                        color: palette.primary.to_iced(),
                        width: 1.5,
                        radius: iced::border::Radius::from(Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.40,
                        ),
                        offset: iced::Vector::new(0.0, 6.0),
                        blur_radius: 24.0,
                    },
                    ..Default::default()
                })
        } else {
            container(main_card_column)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(Spacing::MD)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.07, 0.08, 0.10, 0.96,
                    ))),
                    border: iced::Border {
                        color: palette.primary.to_iced(),
                        width: 1.5,
                        radius: iced::border::Radius::from(Radius::LG),
                    },
                    shadow: iced::Shadow {
                        color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.60),
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
                lang: if lang.is_empty() {
                    "KOD".into()
                } else {
                    lang
                },
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
    fn tier_pill_button<'a>(
        title: &'static str,
        tier_id: &'static str,
        is_active: bool,
        palette: Palette,
    ) -> Element<'a, Message> {
        button(
            container(
                text(title)
                    .size(11.0)
                    .font(iced::Font {
                        weight: if is_active { iced::font::Weight::Semibold } else { iced::font::Weight::Normal },
                        ..Default::default()
                    })
                    .color(if is_active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    })
            )
            .padding([4, 10]),
        )
        .padding(0)
        .style(move |_, status| {
            let bg = if is_active {
                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.22)
            } else {
                match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                }
            };
            let border_color = if is_active {
                palette.primary.to_iced()
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07)
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: border_color,
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            }
        })
        .on_press(Message::SelectModelTier(tier_id.to_string()))
        .into()
    }

    fn chip_button<'a>(label: &'static str, palette: Palette, msg: Message) -> Element<'a, Message> {
        button(
            text(label)
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .on_press(msg)
        .into()
    }

    fn feature_mini_card<'a>(
        title: &'static str,
        desc: &'static str,
        palette: Palette,
    ) -> Element<'a, Message> {
        container(
            column![
                text(title).size(11.5).color(palette.primary.to_iced()),
                text(desc).size(10.0).color(palette.text_muted.to_iced()),
            ]
            .spacing(2),
        )
        .padding([6, 8])
        .width(Length::FillPortion(1))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.03,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        })
        .into()
    }

    fn prompt_chip<'a>(prompt: &'static str, palette: Palette) -> Element<'a, Message> {
        let clean = prompt.trim_matches('"').to_string();
        button(
            text(prompt)
                .size(10.5)
                .color(palette.text_primary.to_iced()),
        )
        .padding([3, 7])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.15,
                ),
                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            }
        })
        .on_press(Message::QuickAction(clean))
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
                        container(text(lang).size(10.0).color(palette.primary.to_iced()))
                            .padding([2, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(
                                    1.0, 1.0, 1.0, 0.08,
                                ))),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        horizontal_space(),
                        button(
                            text(if is_copied {
                                "✓ Kopyalandı!"
                            } else {
                                "📋 Kopyala"
                            })
                            .size(11.0)
                            .color(if is_copied {
                                palette.success.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }),
                        )
                        .padding([2, 8])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                                }
                                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
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
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                    let code_card = container(
                        column![
                            header,
                            vertical_space().height(4),
                            text(code)
                                .size(Typography::BODY_SM)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(4)
                        .width(Length::Fill),
                    )
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            0.04, 0.05, 0.07, 0.95,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    });

                    msg_items = msg_items.push(code_card);
                }
            }
        }

        container(msg_items)
            .padding([10, 14])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.02,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
    }
}
