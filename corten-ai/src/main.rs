use std::borrow::Cow;
use std::sync::Arc;
use tracing::info;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::input_style;
use corten_theme::{Palette, PatinaVariant, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT_BYTES};
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
                println!("corten-ai 0.2.0 (Pure Rust AURA-CORE)");
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

    info!("Starting Corten AI Assistant HUD (corten-ai v0.2.0)...");

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
        fonts: vec![Cow::Borrowed(LUCIDE_FONT_BYTES)],
        layer_settings: LayerShellSettings {
            anchor: Anchor::empty(), // Centered floating window
            layer: Layer::Top,
            margin: (0, 0, 0, 0),
            exclusive_zone: 0,
            size: Some((740, 540)),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            binded_output_name: None,
        },
        default_font: iced::Font::default(),
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

pub struct AppFlags {
    pub initial_query: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    QueryInputChanged(String),
    SubmitQuery,
    QuickAction(String),
    AnalyzeScreenRequested,
    RefreshBackend,
    BackendDetected(BackendType),
    StreamStarted(Arc<Mutex<UnboundedReceiver<StreamChunk>>>),
    StreamChunkReceived(StreamChunk),
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
    history: Vec<ChatMessage>,
    active_stream_content: String,
    stream_rx: Option<Arc<Mutex<UnboundedReceiver<StreamChunk>>>>,
    query_input: String,
    is_loading: bool,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    copied_index: Option<usize>,
    cursor_blink: bool,
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

        let initial_query = flags.initial_query;

        let app = Self {
            backend: BackendType::Offline,
            history: Vec::new(),
            active_stream_content: String::new(),
            stream_rx: None,
            query_input: initial_query.clone().unwrap_or_default(),
            is_loading: initial_query.is_some(),
            theme_mode: ThemeMode::Dark,
            active_patina,
            copied_index: None,
            cursor_blink: true,
        };

        // Initialize backend probe and optionally submit initial query
        let init_task = Task::perform(
            async move {
                let detected = AiEngine::auto_detect().await;
                (detected, initial_query)
            },
            |(detected, opt_q)| {
                if opt_q.is_some() {
                    Message::SubmitQuery
                } else {
                    Message::BackendDetected(detected)
                }
            },
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
            Message::QueryInputChanged(val) => {
                self.query_input = val;
            }
            Message::RefreshBackend => {
                return Task::perform(AiEngine::auto_detect(), Message::BackendDetected);
            }
            Message::BackendDetected(b) => {
                self.backend = b;
            }
            Message::ClearChat => {
                self.history.clear();
                self.active_stream_content.clear();
                self.is_loading = false;
            }
            Message::SubmitQuery => {
                let q = self.query_input.trim().to_string();
                if !q.is_empty() && !self.is_loading {
                    self.is_loading = true;
                    self.active_stream_content.clear();
                    self.query_input.clear();

                    // Push user message to history
                    self.history.push(ChatMessage {
                        role: MessageRole::User,
                        content: q.clone(),
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
                    self.active_stream_content = "Ekran görüntüsü alınıyor ve görsel analiz yapılıyor...".into();

                    let query = "Şu an ekranda ne var? Açık pencereleri, içeriği ve önemli ayrıntıları analiz et.".to_string();
                    self.history.push(ChatMessage {
                        role: MessageRole::User,
                        content: "📸 [Ekran Analiz Talebi]".into(),
                    });

                    let backend = self.backend.clone();
                    let history = self.history.clone();

                    return Task::perform(
                        async move {
                            let screen_base64 = AiEngine::capture_screen_base64();
                            let (tx, rx) = unbounded_channel();
                            tokio::spawn(async move {
                                AiEngine::stream_query(backend, history, query, screen_base64, tx).await;
                            });
                            Arc::new(Mutex::new(rx))
                        },
                        Message::StreamStarted,
                    );
                }
            }
            Message::StreamStarted(rx_arc) => {
                self.active_stream_content.clear();
                self.stream_rx = Some(rx_arc);
            }
            Message::StreamChunkReceived(chunk) => {
                match chunk {
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
                        if !self.active_stream_content.trim().is_empty() {
                            self.history.push(ChatMessage {
                                role: MessageRole::Assistant,
                                content: self.active_stream_content.clone(),
                            });
                        }
                    }
                    StreamChunk::Done => {
                        self.is_loading = false;
                        self.stream_rx = None;
                        if !self.active_stream_content.trim().is_empty() {
                            self.history.push(ChatMessage {
                                role: MessageRole::Assistant,
                                content: self.active_stream_content.clone(),
                            });
                        }
                        self.active_stream_content.clear();
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
            }
            Message::CloseRequested => {
                std::process::exit(0);
            }
            Message::EventTriggered(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            })) => {
                std::process::exit(0);
            }
            Message::EventTriggered(_) => {}
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        // Header: Brand, Backend badge pill, Clear button, Close button
        let (badge_text, badge_color) = match &self.backend {
            BackendType::NativeGguf { model_name, .. } => {
                (format!("● Saf Rust GGUF ({})", model_name), palette.success.to_iced())
            }
            BackendType::Ollama { model, .. } => (format!("● Ollama ({})", model), palette.success.to_iced()),
            BackendType::LlamaCpp { model, .. } => (format!("● llama-server ({})", model), palette.success.to_iced()),
            BackendType::AuraCore { .. } => ("● AURA-CORE (8000)".into(), palette.primary.to_iced()),
            BackendType::Offline => ("○ Çevrimdışı / Vektör & Eylem Modu".into(), palette.warning.to_iced()),
        };

        let badge_btn = button(
            container(
                text(if self.is_loading { "Düşünüyor...".to_string() } else { badge_text })
                    .size(Typography::CAPTION)
                    .color(if self.is_loading { palette.warning.to_iced() } else { badge_color }),
            )
            .padding([3, 8])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06))),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    width: 1.0,
                },
                ..Default::default()
            }),
        )
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Message::RefreshBackend);

        let clear_btn = button(
            text("🧹 Temizle")
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
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
        .on_press(Message::ClearChat);

        let title_row = row![
            text("⚡ Corten AI")
                .size(Typography::TITLE_SM)
                .color(palette.primary.to_iced()),
            badge_btn,
            horizontal_space(),
            clear_btn,
            button(text("✕ ESC").size(Typography::CAPTION).color(palette.text_muted.to_iced()))
                .padding([4, 8])
                .style(|_, _| button::Style::default())
                .on_press(Message::CloseRequested),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        // Chat conversation history + live stream container
        let mut chat_column = column![].spacing(Spacing::MD).padding(Spacing::SM);

        if self.history.is_empty() && self.active_stream_content.is_empty() {
            // Welcome card
            let welcome = container(
                column![
                    text("👋 Merhaba! Ben Corten Desktop Yerel Yapay Zekâ Asistanınız.")
                        .size(Typography::BODY)
                        .color(palette.primary.to_iced()),
                    vertical_space().height(4),
                    text("Tamamen yerel ve gizlilik odaklı mimariyle doğrudan masaüstünüzü yönetebilirim:\n\
                          • \"Sesi %50 yap\" veya \"gece ışığını aç\"\n\
                          • \"Ekran görüntüsü al\" veya \"ekranı analiz et\"\n\
                          • \"En çok RAM kullanan uygulamalar\"\n\
                          • \"3. çalışma alanına geç\"\n\
                          • Kod yazma, hata ayıklama ve sistem analizi...")
                        .size(Typography::BODY_SM)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::MD)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            });

            chat_column = chat_column.push(welcome);
        }

        let mut code_block_idx = 0;

        for msg in &self.history {
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
                            0.20,
                        ))),
                        border: iced::Border {
                            color: palette.primary.to_iced(),
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
                    let rendered = Self::render_rich_message(
                        &msg.content,
                        palette,
                        &mut code_block_idx,
                        self.copied_index,
                    );
                    chat_column = chat_column.push(rendered);
                }
            }
        }

        // Active streaming message
        if !self.active_stream_content.is_empty() || self.is_loading {
            let mut stream_text = self.active_stream_content.clone();
            if self.is_loading {
                stream_text.push_str(if self.cursor_blink { " ▋" } else { "  " });
            }
            let rendered = Self::render_rich_message(
                &stream_text,
                palette,
                &mut code_block_idx,
                self.copied_index,
            );
            chat_column = chat_column.push(rendered);
        }

        let chat_box = container(
            scrollable(chat_column).height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::MD),
            },
            ..Default::default()
        });

        // Quick action chips
        let chips_row = row![
            Self::chip_button("🤖 ReAct Ajan", palette, Message::QuickAction("react: git durumunu ve sistem bilgisini kontrol et".into())),
            Self::chip_button("🧠 Hafıza & Profil", palette, Message::QuickAction("hafıza".into())),
            Self::chip_button("🛠️ Araçlar (20)", palette, Message::QuickAction("araçlar".into())),
            Self::chip_button("🔍 Vektörel Ara", palette, Message::QuickAction("vektör ara: wayland shader".into())),
            Self::chip_button("📸 Ekranı Analiz Et", palette, Message::AnalyzeScreenRequested),
            Self::chip_button("📊 Sistem Durumu", palette, Message::QuickAction("sistem durumu".into())),
        ]
        .spacing(Spacing::XS);

        // Prompt input field & actions
        let screen_btn = button(
            text("📸")
                .size(Typography::BODY_SM)
                .color(palette.text_muted.to_iced()),
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
            "Corten AI'a bir soru sorun, eylem isteyin veya kod analiz edin...",
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
            selection: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4),
        });

        let send_btn = button(
            text(if self.is_loading { "⏳" } else { "Gönder ↵" })
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
        .on_press(Message::SubmitQuery);

        let input_container = container(
            row![screen_btn, input_elem, send_btn]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
        )
        .style(move |_| input_style(palette));

        let main_card = container(
            column![
                title_row,
                vertical_space().height(Spacing::XXS),
                chat_box,
                vertical_space().height(Spacing::XXS),
                chips_row,
                input_container,
            ]
            .spacing(Spacing::SM),
        )
        .width(Length::Fixed(720.0))
        .height(Length::Fixed(520.0))
        .padding(Spacing::LG)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.07, 0.08, 0.10, 0.96))),
            border: iced::Border {
                color: palette.primary.to_iced(),
                width: 1.5,
                radius: iced::border::Radius::from(Radius::LG),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.40),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 18.0,
            },
            ..Default::default()
        });

        container(main_card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen().map(Message::EventTriggered);
        let tick = iced::time::every(std::time::Duration::from_millis(1000)).map(|_| Message::CursorTick);
        let mut subs = vec![events, tick];

        if self.is_loading {
            subs.push(iced::time::every(std::time::Duration::from_millis(350)).map(|_| Message::CursorTick));
        }

        // Live stream subscription
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
                        container(
                            text(lang)
                                .size(10.0)
                                .color(palette.primary.to_iced())
                        )
                        .padding([2, 6])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        horizontal_space(),
                        button(
                            text(if is_copied { "✓ Kopyalandı!" } else { "📋 Kopyala" })
                                .size(11.0)
                                .color(if is_copied { palette.success.to_iced() } else { palette.text_muted.to_iced() })
                        )
                        .padding([2, 8])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
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
                        background: Some(iced::Background::Color(iced::Color::from_rgba(0.04, 0.05, 0.07, 0.95))),
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
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
    }

    fn chip_button<'a>(label: &'static str, palette: Palette, msg: Message) -> Element<'a, Message> {
        button(
            text(label)
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
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
        .on_press(msg)
        .into()
    }
}
