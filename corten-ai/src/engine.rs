use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::timeout;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpTransport {
    Unix(PathBuf),
    Tcp { host: String, port: u16 },
}

impl HttpTransport {
    pub fn host_header(&self) -> String {
        match self {
            Self::Unix(_) => "localhost".to_string(),
            Self::Tcp { host, port } => format!("{host}:{port}"),
        }
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::Unix(path) => format!(
                "UDS ({})",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("aura.sock")
            ),
            Self::Tcp { host, port } => format!("{host}:{port}"),
        }
    }
}

enum TransportStream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl AsyncRead for TransportStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Unix(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TransportStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Unix(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Unix(s) => Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Unix(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn connect_transport(
    transport: &HttpTransport,
    connect_timeout: Duration,
) -> Option<TransportStream> {
    match transport {
        HttpTransport::Unix(path) => {
            if !path.exists() {
                return None;
            }
            timeout(connect_timeout, UnixStream::connect(path))
                .await
                .ok()?
                .ok()
                .map(TransportStream::Unix)
        }
        HttpTransport::Tcp { host, port } => {
            let addr = format!("{host}:{port}");
            timeout(connect_timeout, TcpStream::connect(&addr))
                .await
                .ok()?
                .ok()
                .map(TransportStream::Tcp)
        }
    }
}

pub fn parse_endpoint(raw: &str) -> Option<HttpTransport> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(sock) = trimmed.strip_prefix("unix://") {
        let p = sock.trim();
        if !p.is_empty() {
            return Some(HttpTransport::Unix(PathBuf::from(p)));
        }
        return None;
    }
    if trimmed.starts_with('/') {
        return Some(HttpTransport::Unix(PathBuf::from(trimmed)));
    }
    let stripped = trimmed
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = stripped.split('/').next()?;
    if let Some((host, port_str)) = host_port.split_once(':') {
        let port = port_str.parse::<u16>().ok()?;
        Some(HttpTransport::Tcp {
            host: host.to_string(),
            port,
        })
    } else if !host_port.is_empty() {
        Some(HttpTransport::Tcp {
            host: host_port.to_string(),
            port: 80,
        })
    } else {
        None
    }
}

async fn http_json_request(
    transport: &HttpTransport,
    method: &str,
    path: &str,
    body: Option<&str>,
    io_timeout: Duration,
) -> Option<(u16, String)> {
    let mut stream = connect_transport(transport, Duration::from_millis(600)).await?;
    let host_hdr = transport.host_header();
    let payload = body.unwrap_or("");
    let req = if body.is_some() {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: {host_hdr}\r\nUser-Agent: CortenAI/0.8\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
    } else {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: {host_hdr}\r\nUser-Agent: CortenAI/0.8\r\nConnection: close\r\n\r\n"
        )
    };
    timeout(io_timeout, stream.write_all(req.as_bytes()))
        .await
        .ok()?
        .ok()?;
    let mut buf = Vec::new();
    timeout(io_timeout, stream.read_to_end(&mut buf))
        .await
        .ok()?
        .ok()?;
    let raw = String::from_utf8_lossy(&buf);
    let (headers, body_part) = raw.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let decoded = if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(body_part)
    } else {
        body_part.to_string()
    };
    Some((status, decoded))
}

fn decode_chunked_body(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some((size_hex, after)) = rest.split_once("\r\n") {
        let Ok(len) = usize::from_str_radix(size_hex.trim(), 16) else {
            break;
        };
        if len == 0 || after.len() < len {
            break;
        }
        out.push_str(&after[..len]);
        rest = after.get(len + 2..).unwrap_or("");
    }
    if out.is_empty() { raw.to_string() } else { out }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    pub size_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMode {
    Chat,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendType {
    AuraCore {
        transport: HttpTransport,
        active_model: String,
        models: Vec<ModelEntry>,
    },
    Ollama {
        transport: HttpTransport,
        active_model: String,
        models: Vec<ModelEntry>,
        has_vision: bool,
    },
    OpenAiCompat {
        transport: HttpTransport,
        active_model: String,
        models: Vec<ModelEntry>,
    },
    Offline,
}

impl BackendType {
    pub fn active_model_name(&self) -> Option<&str> {
        match self {
            Self::AuraCore { active_model, .. }
            | Self::Ollama { active_model, .. }
            | Self::OpenAiCompat { active_model, .. } => Some(active_model.as_str()),
            Self::Offline => None,
        }
    }

    pub fn models(&self) -> &[ModelEntry] {
        match self {
            Self::AuraCore { models, .. }
            | Self::Ollama { models, .. }
            | Self::OpenAiCompat { models, .. } => models.as_slice(),
            Self::Offline => &[],
        }
    }

    pub fn supports_agent_mode(&self) -> bool {
        matches!(self, Self::AuraCore { .. })
    }

    pub fn supports_vision(&self) -> bool {
        matches!(
            self,
            Self::Ollama {
                has_vision: true,
                ..
            }
        )
    }

    pub fn status_label(&self) -> String {
        match self {
            Self::AuraCore { transport, .. } => {
                format!("AURA-CORE · {}", transport.display_label())
            }
            Self::Ollama { transport, .. } => format!("Ollama · {}", transport.display_label()),
            Self::OpenAiCompat { transport, .. } => {
                format!("OpenAI API · {}", transport.display_label())
            }
            Self::Offline => "Bağlı yerel sunucu yok".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    #[allow(dead_code)]
    System,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub thought: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Token(String),
    Thought(String),
    Metrics(f32),
    Done,
    Error(String),
}

/// Isolates `<think>...</think>` reasoning blocks from visible assistant text.
pub struct ThoughtStreamFilter {
    in_think: bool,
}

impl ThoughtStreamFilter {
    pub fn new() -> Self {
        Self { in_think: false }
    }

    pub fn process(&mut self, token: &str, tx: &UnboundedSender<StreamChunk>) {
        let mut cur = token;
        while !cur.is_empty() {
            if self.in_think {
                if let Some(pos) = cur.find("</think>") {
                    let thought_part = &cur[..pos];
                    if !thought_part.is_empty() {
                        let _ = tx.send(StreamChunk::Thought(thought_part.to_string()));
                    }
                    self.in_think = false;
                    cur = &cur[pos + 8..];
                } else {
                    let _ = tx.send(StreamChunk::Thought(cur.to_string()));
                    break;
                }
            } else if let Some(pos) = cur.find("<think>") {
                let token_part = &cur[..pos];
                if !token_part.is_empty() {
                    let _ = tx.send(StreamChunk::Token(token_part.to_string()));
                }
                self.in_think = true;
                cur = &cur[pos + 7..];
            } else {
                let _ = tx.send(StreamChunk::Token(cur.to_string()));
                break;
            }
        }
    }
}

pub struct AiEngine;

fn format_bytes_gb(bytes: u64) -> String {
    if bytes == 0 {
        return String::new();
    }
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        format!("{mb:.0} MB")
    }
}

pub(crate) fn parse_aura_models_json(raw: &str) -> Vec<ModelEntry> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let arr = val
        .get("models")
        .and_then(|m| m.as_array())
        .or_else(|| val.as_array());
    let Some(items) = arr else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("id")
                .or_else(|| item.get("model_id"))
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())?
                .to_string();
            let label = item
                .get("name")
                .or_else(|| item.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let size_label = item
                .get("size_gb")
                .and_then(|v| v.as_f64())
                .map(|gb| format!("{gb:.1} GB"))
                .or_else(|| {
                    item.get("size_bytes")
                        .or_else(|| item.get("size"))
                        .and_then(|v| v.as_u64())
                        .map(format_bytes_gb)
                })
                .unwrap_or_default();
            Some(ModelEntry {
                id,
                label,
                size_label,
            })
        })
        .collect()
}

fn is_vision_model_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("vl")
        || lower.contains("vision")
        || lower.contains("llava")
        || lower.contains("minicpm-v")
        || lower.contains("moondream")
        || lower.contains("gemma3")
}

impl AiEngine {
    /// Probes an endpoint to see if it is a genuine AURA-CORE server (never misidentifies
    /// unrelated developer servers on `:8000`).
    async fn probe_aura(transport: &HttpTransport, preferred_model: &str) -> Option<BackendType> {
        let (status, active_body) = http_json_request(
            transport,
            "GET",
            "/models/active",
            None,
            Duration::from_millis(800),
        )
        .await?;
        if status != 200 {
            return None;
        }
        let active_val = serde_json::from_str::<serde_json::Value>(&active_body).ok()?;
        let active_from_server = active_val
            .get("id")
            .or_else(|| active_val.get("model_id"))
            .or_else(|| active_val.get("name"))
            .or_else(|| active_val.get("active_model"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string)?;

        let models = match http_json_request(
            transport,
            "GET",
            "/models",
            None,
            Duration::from_millis(1000),
        )
        .await
        {
            Some((200, body)) => parse_aura_models_json(&body),
            _ => Vec::new(),
        };

        let mut active_model = active_from_server;
        if !preferred_model.is_empty()
            && preferred_model != active_model
            && models.iter().any(|m| m.id == preferred_model)
        {
            let body = serde_json::json!({ "model_id": preferred_model }).to_string();
            if let Some((200, _)) = http_json_request(
                transport,
                "POST",
                "/models/switch",
                Some(&body),
                Duration::from_secs(3),
            )
            .await
            {
                active_model = preferred_model.to_string();
            }
        }

        info!(
            endpoint = %transport.display_label(),
            model = %active_model,
            total_models = models.len(),
            "Corten AI connected to AURA-CORE"
        );
        Some(BackendType::AuraCore {
            transport: transport.clone(),
            active_model,
            models,
        })
    }

    async fn probe_ollama(transport: &HttpTransport, preferred_model: &str) -> Option<BackendType> {
        let (status, body) = http_json_request(
            transport,
            "GET",
            "/api/tags",
            None,
            Duration::from_millis(800),
        )
        .await?;
        if status != 200 {
            return None;
        }
        let val = serde_json::from_str::<serde_json::Value>(&body).ok()?;
        let models_arr = val.get("models")?.as_array()?;
        let models: Vec<ModelEntry> = models_arr
            .iter()
            .filter_map(|m| {
                let id = m.get("name")?.as_str()?.to_string();
                let size_label = m
                    .get("size")
                    .and_then(|v| v.as_u64())
                    .map(format_bytes_gb)
                    .unwrap_or_default();
                Some(ModelEntry {
                    label: id.clone(),
                    id,
                    size_label,
                })
            })
            .collect();
        if models.is_empty() {
            return None;
        }
        let active_model =
            if !preferred_model.is_empty() && models.iter().any(|m| m.id == preferred_model) {
                preferred_model.to_string()
            } else {
                models[0].id.clone()
            };
        let has_vision = is_vision_model_name(&active_model);
        Some(BackendType::Ollama {
            transport: transport.clone(),
            active_model,
            models,
            has_vision,
        })
    }

    async fn probe_openai_compat(
        transport: &HttpTransport,
        preferred_model: &str,
    ) -> Option<BackendType> {
        let (status, body) = http_json_request(
            transport,
            "GET",
            "/v1/models",
            None,
            Duration::from_millis(800),
        )
        .await?;
        if status != 200 {
            return None;
        }
        let val = serde_json::from_str::<serde_json::Value>(&body).ok()?;
        let data = val.get("data")?.as_array()?;
        let models: Vec<ModelEntry> = data
            .iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                Some(ModelEntry {
                    label: id.clone(),
                    id,
                    size_label: String::new(),
                })
            })
            .collect();
        if models.is_empty() {
            return None;
        }
        let active_model =
            if !preferred_model.is_empty() && models.iter().any(|m| m.id == preferred_model) {
                preferred_model.to_string()
            } else {
                models[0].id.clone()
            };
        Some(BackendType::OpenAiCompat {
            transport: transport.clone(),
            active_model,
            models,
        })
    }

    pub async fn auto_detect_with_config(
        config: Option<&corten_config::AiSettingsConfig>,
    ) -> BackendType {
        let default_cfg = corten_config::AiSettingsConfig::default();
        let cfg = config.unwrap_or(&default_cfg);
        if !cfg.enabled {
            return BackendType::Offline;
        }
        let preferred = cfg.selected_model.trim();

        // 1. Prefer session Unix Domain Socket ($XDG_RUNTIME_DIR/corten/aura.sock — zero TCP ports)
        let uds = HttpTransport::Unix(corten_config::default_aura_socket_path());
        if let Some(found) = Self::probe_aura(&uds, preferred).await {
            return found;
        }

        // 2. Configured backend_url from ai.toml (supports unix:// or http://host:port)
        if let Some(custom) = parse_endpoint(&cfg.backend_url) {
            if let Some(found) = Self::probe_aura(&custom, preferred).await {
                return found;
            }
            if let Some(found) = Self::probe_ollama(&custom, preferred).await {
                return found;
            }
            if let Some(found) = Self::probe_openai_compat(&custom, preferred).await {
                return found;
            }
        }

        // 3. Dedicated Corten AURA TCP port (11435), then manually started AURA (8000, verified)
        for port in [11435u16, 8000u16] {
            let t = HttpTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            };
            if let Some(found) = Self::probe_aura(&t, preferred).await {
                return found;
            }
        }

        // 4. Standard Ollama (11434) and llama-server (8080)
        let ollama_t = HttpTransport::Tcp {
            host: "127.0.0.1".to_string(),
            port: 11434,
        };
        if let Some(found) = Self::probe_ollama(&ollama_t, preferred).await {
            return found;
        }

        let llama_t = HttpTransport::Tcp {
            host: "127.0.0.1".to_string(),
            port: 8080,
        };
        if let Some(found) = Self::probe_openai_compat(&llama_t, preferred).await {
            return found;
        }

        BackendType::Offline
    }

    /// Hot-switches the active model on AURA-CORE or updates selection on Ollama/OpenAI.
    pub async fn switch_model(backend: BackendType, model_id: String) -> BackendType {
        match backend {
            BackendType::AuraCore {
                transport,
                mut active_model,
                models,
            } => {
                let body = serde_json::json!({ "model_id": model_id }).to_string();
                if let Some((200, _)) = http_json_request(
                    &transport,
                    "POST",
                    "/models/switch",
                    Some(&body),
                    Duration::from_secs(5),
                )
                .await
                {
                    active_model = model_id;
                }
                BackendType::AuraCore {
                    transport,
                    active_model,
                    models,
                }
            }
            BackendType::Ollama {
                transport, models, ..
            } => {
                let has_vision = is_vision_model_name(&model_id);
                BackendType::Ollama {
                    transport,
                    active_model: model_id,
                    models,
                    has_vision,
                }
            }
            BackendType::OpenAiCompat {
                transport, models, ..
            } => BackendType::OpenAiCompat {
                transport,
                active_model: model_id,
                models,
            },
            BackendType::Offline => BackendType::Offline,
        }
    }

    /// Starts the local AURA-CORE server bound to `$XDG_RUNTIME_DIR/corten/aura.sock` (zero TCP port usage).
    pub fn launch_aura_daemon_uds() -> Result<PathBuf, String> {
        let sock = corten_config::default_aura_socket_path();
        if let Some(parent) = sock.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&sock);

        let project_dir = std::env::var("AURA_PROJECT_DIR")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .or_else(|| {
                let home = std::env::var("HOME").ok()?;
                let candidate = PathBuf::from(home).join("Projects/AIProject");
                candidate.exists().then_some(candidate)
            });

        if let Some(dir) = project_dir {
            let venv_py = dir.join(".venv/bin/python");
            let app_dir = dir.join("Rust-Python-Vector-Engine");
            if venv_py.exists() && app_dir.exists() {
                std::process::Command::new(&venv_py)
                    .args([
                        "-m",
                        "uvicorn",
                        "api_server.app.main:app",
                        "--uds",
                        &sock.to_string_lossy(),
                        "--app-dir",
                        &app_dir.to_string_lossy(),
                    ])
                    .env("AURA_TOOL_PROFILE", "corten")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("AURA sunucusu başlatılamadı: {e}"))?;
                return Ok(sock);
            }
        }

        Err(
            "AIProject (.venv) bulunamadı. AURA_PROJECT_DIR veya Ollama/llama-server başlatın."
                .into(),
        )
    }

    pub async fn stream_query(
        backend: BackendType,
        mode: ChatMode,
        history: Vec<ChatMessage>,
        prompt: String,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        match backend {
            BackendType::AuraCore {
                transport,
                active_model,
                ..
            } => {
                Self::stream_aura_core(
                    &transport,
                    &active_model,
                    mode,
                    &history,
                    &prompt,
                    cancel,
                    tx,
                )
                .await;
            }
            BackendType::Ollama {
                transport,
                active_model,
                ..
            } => {
                Self::stream_ollama(&transport, &active_model, &history, cancel, tx).await;
            }
            BackendType::OpenAiCompat {
                transport,
                active_model,
                ..
            } => {
                Self::stream_openai_compat(&transport, &active_model, &history, cancel, tx).await;
            }
            BackendType::Offline => {
                let _ = tx.send(StreamChunk::Error(
                    "Yerel AI sunucusu bağlı değil. Üstteki 'Sunucuyu Başlat' ile AURA-CORE'u (UDS) açabilir veya Ollama başlatabilirsiniz."
                        .into(),
                ));
                let _ = tx.send(StreamChunk::Done);
            }
        }
    }

    async fn stream_aura_core(
        transport: &HttpTransport,
        active_model: &str,
        mode: ChatMode,
        history: &[ChatMessage],
        prompt: &str,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let path = match mode {
            ChatMode::Chat => "/agent/chat/stream",
            ChatMode::Agent => "/agent/react/stream",
        };
        let body = serde_json::json!({
            "query": prompt,
            "model": active_model,
            "max_tokens": 1024
        })
        .to_string();

        if Self::stream_sse(transport, path, &body, &cancel, &tx).await {
            let _ = tx.send(StreamChunk::Done);
            return;
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // Fallback to AURA-CORE's OpenAI-compatible streaming endpoint with full history
        Self::stream_openai_compat(transport, active_model, history, cancel, tx).await;
    }

    async fn stream_sse(
        transport: &HttpTransport,
        path: &str,
        body: &str,
        cancel: &Arc<AtomicBool>,
        tx: &UnboundedSender<StreamChunk>,
    ) -> bool {
        let Some(mut stream) = connect_transport(transport, Duration::from_secs(2)).await else {
            return false;
        };
        let host_hdr = transport.host_header();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {host_hdr}\r\nUser-Agent: CortenAI/0.8\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if stream.write_all(req.as_bytes()).await.is_err() {
            return false;
        }

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        let mut in_headers = true;
        let mut status_ok = false;
        let mut got_any = false;
        let mut token_count: u32 = 0;
        let started = Instant::now();
        let mut filter = ThoughtStreamFilter::new();

        while let Ok(Ok(Some(line))) = timeout(Duration::from_secs(45), lines.next_line()).await {
            if cancel.load(Ordering::Relaxed) {
                return true;
            }
            if in_headers {
                if !status_ok && line.starts_with("HTTP/") {
                    status_ok = line.contains(" 200 ");
                }
                if line.trim().is_empty() {
                    if !status_ok {
                        return false;
                    }
                    in_headers = false;
                }
                continue;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed == "data: [DONE]" {
                if trimmed == "data: [DONE]" {
                    break;
                }
                continue;
            }
            if trimmed.chars().all(|c| c.is_ascii_hexdigit()) && trimmed.len() <= 8 {
                continue;
            }

            let json_payload = trimmed.strip_prefix("data: ").unwrap_or(trimmed);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_payload) {
                if let Some(tok) = val
                    .get("token")
                    .or_else(|| val.get("content"))
                    .or_else(|| val.get("delta"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    got_any = true;
                    token_count += 1;
                    filter.process(tok, tx);
                } else if let Some(tok) = val
                    .pointer("/choices/0/delta/content")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    got_any = true;
                    token_count += 1;
                    filter.process(tok, tx);
                } else if let Some(thought) = val
                    .get("thought")
                    .or_else(|| val.get("reasoning"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    got_any = true;
                    let _ = tx.send(StreamChunk::Thought(thought.to_string()));
                } else if let Some(ans) = val
                    .get("answer")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    got_any = true;
                    filter.process(ans, tx);
                }

                let secs = started.elapsed().as_secs_f32();
                if token_count > 0 && secs > 0.15 {
                    let _ = tx.send(StreamChunk::Metrics(token_count as f32 / secs));
                }
            }
        }
        got_any
    }

    async fn stream_openai_compat(
        transport: &HttpTransport,
        active_model: &str,
        history: &[ChatMessage],
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let mut msgs = vec![serde_json::json!({
            "role": "system",
            "content": "Sen Corten Desktop masaüstü asistanısın. Net, doğru ve doğrudan yanıt ver."
        })];
        for m in history.iter().rev().take(12).rev() {
            let role = match m.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            };
            msgs.push(serde_json::json!({
                "role": role,
                "content": m.content,
            }));
        }
        let body = serde_json::json!({
            "model": active_model,
            "messages": msgs,
            "stream": true,
            "temperature": 0.4
        })
        .to_string();

        if !Self::stream_sse(transport, "/v1/chat/completions", &body, &cancel, &tx).await
            && !cancel.load(Ordering::Relaxed)
        {
            let _ = tx.send(StreamChunk::Error(
                "Sunucudan yanıt alınamadı. Modelin yüklü ve sunucunun çalışır olduğunu kontrol edin."
                    .into(),
            ));
        }
        let _ = tx.send(StreamChunk::Done);
    }

    async fn stream_ollama(
        transport: &HttpTransport,
        active_model: &str,
        history: &[ChatMessage],
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let Some(mut stream) = connect_transport(transport, Duration::from_secs(2)).await else {
            let _ = tx.send(StreamChunk::Error(
                "Ollama sunucusuna bağlanılamadı.".into(),
            ));
            let _ = tx.send(StreamChunk::Done);
            return;
        };

        let mut msgs = vec![serde_json::json!({
            "role": "system",
            "content": "Sen Corten Desktop masaüstü asistanısın. Net, doğru ve doğrudan yanıt ver."
        })];
        for m in history.iter().rev().take(12).rev() {
            let role = match m.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            };
            msgs.push(serde_json::json!({ "role": role, "content": m.content }));
        }
        let body = serde_json::json!({
            "model": active_model,
            "messages": msgs,
            "stream": true
        })
        .to_string();

        let host_hdr = transport.host_header();
        let req = format!(
            "POST /api/chat HTTP/1.1\r\nHost: {host_hdr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if stream.write_all(req.as_bytes()).await.is_err() {
            let _ = tx.send(StreamChunk::Error("Ollama isteği gönderilemedi.".into()));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        let mut in_headers = true;
        let mut token_count: u32 = 0;
        let started = Instant::now();
        let mut filter = ThoughtStreamFilter::new();

        while let Ok(Ok(Some(line))) = timeout(Duration::from_secs(45), lines.next_line()).await {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if in_headers {
                if line.trim().is_empty() {
                    in_headers = false;
                }
                continue;
            }
            let trimmed = line.trim();
            if trimmed.is_empty()
                || (trimmed.chars().all(|c| c.is_ascii_hexdigit()) && trimmed.len() <= 8)
            {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(tok) = val
                    .pointer("/message/content")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    token_count += 1;
                    filter.process(tok, &tx);
                    let secs = started.elapsed().as_secs_f32();
                    if secs > 0.15 {
                        let _ = tx.send(StreamChunk::Metrics(token_count as f32 / secs));
                    }
                }
                if val.get("done").and_then(|d| d.as_bool()) == Some(true) {
                    break;
                }
            }
        }
        let _ = tx.send(StreamChunk::Done);
    }

    pub fn copy_to_clipboard(text: &str) {
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }
    }

    #[allow(dead_code)]
    pub fn socket_exists(path: &Path) -> bool {
        path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpTransport, parse_aura_models_json, parse_endpoint};
    use std::path::PathBuf;

    #[test]
    fn parses_uds_and_tcp_endpoints() {
        assert_eq!(
            parse_endpoint("unix:///run/user/1000/corten/aura.sock"),
            Some(HttpTransport::Unix(PathBuf::from(
                "/run/user/1000/corten/aura.sock"
            )))
        );
        assert_eq!(
            parse_endpoint("/tmp/corten-aura.sock"),
            Some(HttpTransport::Unix(PathBuf::from("/tmp/corten-aura.sock")))
        );
        assert_eq!(
            parse_endpoint("http://127.0.0.1:11435"),
            Some(HttpTransport::Tcp {
                host: "127.0.0.1".into(),
                port: 11435
            })
        );
    }

    #[test]
    fn parses_dynamic_aura_models_of_any_size() {
        let json = r#"{
            "models": [
                {"id": "qwen2.5-coder-3b", "name": "Qwen2.5 Coder 3B", "size_gb": 2.1},
                {"id": "qwen2.5-32b-q4", "name": "Qwen2.5 32B Q4_K_M", "size_gb": 19.8}
            ]
        }"#;
        let models = parse_aura_models_json(json);
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].id, "qwen2.5-32b-q4");
        assert_eq!(models[1].size_label, "19.8 GB");
    }
}
