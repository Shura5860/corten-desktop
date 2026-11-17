use crate::actions::{ActionExecutor, DesktopAction};
use crate::aura::{
    extract_math_candidate, AuraMemoryEngine, AuraReActAgent, AuraToolRegistry, AuraWorkspaceRag,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::timeout;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub enum BackendType {
    NativeGguf {
        model_path: std::path::PathBuf,
        model_name: String,
    },
    Ollama {
        host: String,
        port: u16,
        model: String,
        tier_label: String,
        all_models: Vec<String>,
        has_vision: bool,
    },
    LlamaCpp {
        host: String,
        port: u16,
        model: String,
    },
    AuraCore {
        host: String,
        port: u16,
    },
    Offline,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum MessageRole {
    User,
    Assistant,
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
    ActionExecuted(String),
    Done,
    Error(String),
}

/// Helper that filters out `<think>...</think>` tags from model token streams
/// and cleanly isolates reasoning thoughts from actual assistant responses.
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
                    cur = &cur[pos + 8..]; // skip "</think>"
                } else {
                    let _ = tx.send(StreamChunk::Thought(cur.to_string()));
                    break;
                }
            } else {
                if let Some(pos) = cur.find("<think>") {
                    let token_part = &cur[..pos];
                    if !token_part.is_empty() {
                        let _ = tx.send(StreamChunk::Token(token_part.to_string()));
                    }
                    self.in_think = true;
                    cur = &cur[pos + 7..]; // skip "<think>"
                } else {
                    let _ = tx.send(StreamChunk::Token(cur.to_string()));
                    break;
                }
            }
        }
    }
}

pub struct AiEngine {
    #[allow(dead_code)]
    pub backend: BackendType,
}

impl Default for AiEngine {
    fn default() -> Self {
        Self {
            backend: BackendType::Offline,
        }
    }
}

fn parse_url_host_port(url_str: &str) -> Option<(String, u16)> {
    let s = url_str
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = s.split('/').next()?;
    if let Some((h, p)) = host_port.split_once(':') {
        let port: u16 = p.parse().ok()?;
        Some((h.to_string(), port))
    } else {
        Some((host_port.to_string(), 80))
    }
}

impl AiEngine {
    /// Detect which local AI backend is running, respecting user settings from `ai.toml`
    pub async fn auto_detect() -> BackendType {
        let cfg = corten_config::ConfigManager::<corten_config::AiSettingsConfig>::load_or_default("ai")
            .map(|c| c.get().clone())
            .unwrap_or_default();
        Self::auto_detect_with_config(Some(&cfg)).await
    }

    pub async fn auto_detect_with_config(config: Option<&corten_config::AiSettingsConfig>) -> BackendType {
        let (enabled, target_tier, custom_path, backend_url) = if let Some(c) = config {
            (
                c.enabled,
                c.model_tier.as_str(),
                c.custom_model_path.as_str(),
                c.backend_url.as_str(),
            )
        } else {
            (true, "eco", "", "http://127.0.0.1:8000")
        };

        if !enabled {
            info!("Corten AI: Disabled in system settings (`ai.toml`)");
            return BackendType::Offline;
        }

        // 1. Check custom model path if specified in settings
        if !custom_path.is_empty() {
            let p = std::path::PathBuf::from(custom_path);
            if p.exists() && p.is_file() {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("custom-gguf")
                    .to_string();
                info!("Corten AI: Using custom model path from settings: {:?}", p);
                return BackendType::NativeGguf {
                    model_path: p,
                    model_name: name,
                };
            }
        }

        // 2. Check in-process Native Rust GGUF models (0 external dependency, 100% pure Rust)
        let local_ggufs = corten_llm::discover_local_gguf_models();
        if let Some(first_gguf) = local_ggufs.first() {
            let model_name = first_gguf
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("local-gguf")
                .to_string();
            info!("Corten AI: Discovered Native Rust GGUF model '{}'", model_name);
            return BackendType::NativeGguf {
                model_path: first_gguf.clone(),
                model_name,
            };
        }

        // 3. If configured backend_url points to a specific endpoint, probe it first
        if let Some((host, port)) = parse_url_host_port(backend_url) {
            if port == 8000 {
                if let Ok(info) = Self::probe_aura_core(&host, port).await {
                    return info;
                }
            } else if port == 11434 {
                if let Ok(info) = Self::probe_ollama(&host, port, Some(target_tier)).await {
                    return info;
                }
            } else if port == 8080 {
                if let Ok(info) = Self::probe_llama_cpp(&host, port).await {
                    return info;
                }
            }
        }

        // 4. Probe Ollama on standard port (11434)
        if let Ok(info) = Self::probe_ollama("127.0.0.1", 11434, Some(target_tier)).await {
            return info;
        }

        // 5. Probe llama.cpp on standard port (8080)
        if let Ok(info) = Self::probe_llama_cpp("127.0.0.1", 8080).await {
            return info;
        }

        // 6. Probe AURA-CORE on standard port (8000)
        if let Ok(info) = Self::probe_aura_core("127.0.0.1", 8000).await {
            return info;
        }

        BackendType::Offline
    }

    pub fn classify_model_tier(name: &str) -> &'static str {
        let lower = name.to_lowercase();
        if lower.contains("1.5b")
            || lower.contains("1b")
            || lower.contains("2b")
            || lower.contains("tiny")
            || lower.contains("mini")
        {
            "Eco (1.5B)"
        } else if lower.contains("3b") || lower.contains("4b") || lower.contains("medium") {
            "Dengeli (3B)"
        } else if lower.contains("7b")
            || lower.contains("8b")
            || lower.contains("14b")
            || lower.contains("large")
        {
            "Pro (7B)"
        } else {
            "Özel Model"
        }
    }

    async fn probe_ollama(
        host: &str,
        port: u16,
        target_tier: Option<&str>,
    ) -> Result<BackendType, ()> {
        let addr = format!("{}:{}", host, port);
        let mut stream = timeout(Duration::from_millis(400), TcpStream::connect(&addr))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;

        let req = format!(
            "GET /api/tags HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            host, port
        );
        stream.write_all(req.as_bytes()).await.map_err(|_| ())?;

        let mut buf = Vec::new();
        timeout(Duration::from_millis(600), stream.read_to_end(&mut buf))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;

        let resp = String::from_utf8_lossy(&buf);
        if let Some(pos) = resp.find("\r\n\r\n") {
            let body = &resp[pos + 4..];
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                if let Some(models) = json.get("models").and_then(|m| m.as_array()) {
                    let mut names = Vec::new();
                    let mut has_vision = false;
                    for m in models {
                        if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                            let lower = name.to_lowercase();
                            if lower.contains("llava")
                                || lower.contains("moondream")
                                || lower.contains("vision")
                                || lower.contains("minicpm")
                                || lower.contains("qwen2-vl")
                            {
                                has_vision = true;
                            }
                            names.push(name.to_string());
                        }
                    }
                    if !names.is_empty() {
                        let desired = target_tier.unwrap_or("eco");
                        let chosen_idx = names
                            .iter()
                            .position(|n| {
                                let tier_str = Self::classify_model_tier(n);
                                match desired {
                                    "eco" => tier_str.contains("Eco"),
                                    "balanced" => tier_str.contains("Dengeli"),
                                    "pro" => tier_str.contains("Pro"),
                                    _ => false,
                                }
                            })
                            .unwrap_or(0);

                        let active = names[chosen_idx].clone();
                        let tier_label = Self::classify_model_tier(&active).to_string();
                        info!(
                            "Corten AI: Found Ollama backend with model '{}' [{}] (requested tier: {})",
                            active, tier_label, desired
                        );
                        return Ok(BackendType::Ollama {
                            host: host.to_string(),
                            port,
                            model: active,
                            tier_label,
                            all_models: names,
                            has_vision,
                        });
                    }
                }
            }
        }
        Err(())
    }

    async fn probe_llama_cpp(host: &str, port: u16) -> Result<BackendType, ()> {
        let addr = format!("{}:{}", host, port);
        let mut stream = timeout(Duration::from_millis(300), TcpStream::connect(&addr))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;

        let req = format!(
            "GET /v1/models HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            host, port
        );
        stream.write_all(req.as_bytes()).await.map_err(|_| ())?;

        let mut buf = Vec::new();
        timeout(Duration::from_millis(500), stream.read_to_end(&mut buf))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;

        let resp = String::from_utf8_lossy(&buf);
        if let Some(pos) = resp.find("\r\n\r\n") {
            let body = &resp[pos + 4..];
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                let model_name = json
                    .get("data")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|m| m.get("id"))
                    .and_then(|id| id.as_str())
                    .unwrap_or("llama-server")
                    .to_string();
                return Ok(BackendType::LlamaCpp {
                    host: host.to_string(),
                    port,
                    model: model_name,
                });
            }
        }
        Err(())
    }

    async fn probe_aura_core(host: &str, port: u16) -> Result<BackendType, ()> {
        let addr = format!("{}:{}", host, port);
        let _ = timeout(Duration::from_millis(300), TcpStream::connect(&addr))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        Ok(BackendType::AuraCore {
            host: host.to_string(),
            port,
        })
    }

    /// Stream queries with multi-turn history, screen understanding, and reflex actions
    pub async fn stream_query(
        backend: BackendType,
        history: Vec<ChatMessage>,
        query: String,
        screen_image: Option<String>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let trimmed = query.trim().to_string();
        if trimmed.is_empty() {
            let _ = tx.send(StreamChunk::Error("Lütfen bir komut veya soru girin.".into()));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 0. AURA-CORE Long-Term Memory & Profile Learning Interceptor (`memory_router` parity)
        if let Some(learned_msg) = AuraMemoryEngine::inspect_and_learn(&trimmed) {
            let _ = tx.send(StreamChunk::Token(learned_msg));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        let lower_q = trimmed.to_lowercase();

        // 0.1. Memory & Profile Status Inspection
        if lower_q == "hafıza"
            || lower_q == "profilim"
            || lower_q == "anılarım"
            || lower_q == "hafızayı göster"
        {
            let report = AuraMemoryEngine::format_memory_status();
            let _ = tx.send(StreamChunk::Token(report));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 0.2. 20-Tool Registry Catalog Inspection (`agent/tools` parity)
        if lower_q == "araçlar"
            || lower_q == "tools"
            || lower_q == "yetenekler"
            || lower_q == "ajan araçları"
        {
            let catalog = AuraToolRegistry::format_tools_catalog();
            let _ = tx.send(StreamChunk::Token(catalog));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 0.3. Pure-Rust Calculator Tool (`calculator` parity)
        if let Some(math_expr) = extract_math_candidate(&trimmed) {
            let calc_res = AuraToolRegistry::execute_tool("calculator", &math_expr);
            let _ = tx.send(StreamChunk::Token(calc_res));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 0.4. Multi-Step Autonomous ReAct Agent Workflow (`agent/react` parity)
        if lower_q.starts_with("react:")
            || lower_q.starts_with("react ")
            || lower_q.starts_with("ajan:")
            || lower_q.starts_with("ajan ")
            || lower_q.starts_with("otonom:")
        {
            let report = AuraReActAgent::execute_react_workflow(&trimmed);
            let _ = tx.send(StreamChunk::Token(report));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 1. Reflex Action Interceptor
        let intent = ActionExecutor::parse_intent(&trimmed);
        if intent != DesktopAction::None {
            if let Some(feedback) = ActionExecutor::execute(intent).await {
                let _ = tx.send(StreamChunk::ActionExecuted(feedback.clone()));
                let _ = tx.send(StreamChunk::Token(feedback));
                let _ = tx.send(StreamChunk::Done);
                return;
            }
        }

        // 2. Local Heuristic Static Security Sentinel
        if trimmed.contains("siber güvenlik")
            || trimmed.contains("Güvenlik")
            || trimmed.contains("güvenlik")
            || trimmed.contains("audit")
        {
            let report = Self::heuristic_security_audit(&trimmed);
            let _ = tx.send(StreamChunk::Token(report));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 3. In-Process Semantic Vector Search (`corten-vector` HNSW Engine)
        if lower_q.starts_with("dosya ara")
            || lower_q.starts_with("belge bul")
            || lower_q.starts_with("kod ara")
            || lower_q.starts_with("vektör ara")
            || lower_q.contains("vektörel dosya ara")
        {
            let report = Self::semantic_desktop_search(&trimmed);
            let _ = tx.send(StreamChunk::Token(report));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        // 4. Dispatch to Detected Local Engine
        match backend {
            BackendType::NativeGguf { model_path, .. } => {
                Self::stream_native_gguf(&model_path, &trimmed, tx).await;
            }
            BackendType::Ollama {
                host,
                port,
                model,
                has_vision,
                ..
            } => {
                let use_image = if has_vision { screen_image } else { None };
                Self::stream_ollama(&host, port, &model, &history, &trimmed, use_image, tx).await;
            }
            BackendType::LlamaCpp { host, port, model } => {
                Self::stream_llama_cpp(&host, port, &model, &history, &trimmed, tx).await;
            }
            BackendType::AuraCore { host, port } => {
                Self::stream_aura_core(&host, port, &trimmed, tx).await;
            }
            BackendType::Offline => {
                // In offline mode, provide a clear, helpful desktop assistant response instead of synthetic raw trace dumping
                let response = Self::offline_assistant_response(&trimmed);
                let _ = tx.send(StreamChunk::Token(response));
                let _ = tx.send(StreamChunk::Done);
            }
        }
    }

    /// Clean, helpful assistant response when no external LLM or GGUF model is loaded
    pub fn offline_assistant_response(query: &str) -> String {
        format!(
            "⚡ **Corten AI Sistem Asistanı (Yerel Mod)**\n\n\
            *Sorgu:* \"{}\"\n\n\
            Yerel nöral LLM motoru şu anda bağlı değil veya çevrimdışı modda. Sistem asistanı olarak doğrudan yapabileceğim masaüstü eylemleri:\n\n\
            • 🔊 **Ses & Parlaklık:** `\"sesi %60 yap\"`, `\"ekran ışığını kıs\"`\n\
            • 📸 **Ekran & Pencereler:** `\"ekran görüntüsü al\"`, `\"tam ekran yap\"`, `\"pencereleri küçült\"`\n\
            • 📂 **Dosya Arama (Vektör):** `\"dosya ara: <kelime>\"`\n\
            • 📊 **Sistem Telemetrisi:** `\"sistem durumu\"`, `\"en çok ram kullanan\"`\n\
            • 🧮 **Hesap Makinesi:** `\"hesapla: (144 * 12) / 4\"`\n\
            • 🧠 **Hafıza & Not:** `\"hatırla: API anahtarı yenilenecek\"`\n\n\
            💡 *Tam metin sohbet ve kod üretimi için Sistem Ayarları'ndan (`Super+Alt+Space -> Ayarlar`) yerel Ollama veya GGUF modelini bağlayabilirsiniz.*",
            query
        )
    }

    async fn stream_native_gguf(
        model_path: &std::path::Path,
        prompt: &str,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let path_buf = model_path.to_path_buf();
        let memory_ctx = AuraMemoryEngine::recall_context_for_prompt(prompt, 3);
        let relevant_tools = AuraToolRegistry::get_relevant_tools(prompt, 6);
        let tools_prompt = AuraToolRegistry::format_tools_prompt(&relevant_tools);
        let rag_hits = AuraWorkspaceRag::retrieve_rag_context(prompt, 2, None);
        let rag_ctx = if rag_hits.is_empty() {
            String::new()
        } else {
            let docs = rag_hits
                .iter()
                .map(|r| {
                    format!(
                        "- {}: {}",
                        r.metadata.get("name").cloned().unwrap_or_default(),
                        r.metadata.get("snippet").cloned().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n[YEREL BELGE BAĞLAMI (RAG)]:\n{}", docs)
        };
        let prompt_str = format!(
            "<|im_start|>system\nSen Corten Desktop sistem ve masaüstü otomasyon asistanısın (AURA-CORE). Türkçe, net ve doğrudan eylem odaklı yanıt ver.\n[KULLANICI HAFIZASI]:\n{}\n[KULLANILABİLİR ARAÇLAR]:\n{}{}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            memory_ctx, tools_prompt, rag_ctx, prompt
        );
        let tx_clone = tx.clone();

        let _ = tokio::task::spawn_blocking(move || {
            match corten_llm::ModelRunner::load_from_file(&path_buf) {
                Ok(runner) => {
                    let params = corten_llm::SamplerParams {
                        temperature: 0.3,
                        top_k: 40,
                        top_p: 0.9,
                        repetition_penalty: 1.1,
                    };
                    let (std_tx, std_rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = runner.generate_tui_stream(&prompt_str, 256, &params, &std_tx);
                    });

                    while let Ok(msg) = std_rx.recv() {
                        match msg {
                            corten_llm::InferenceMessage::Token(t) => {
                                let _ = tx_clone.send(StreamChunk::Token(t));
                            }
                            corten_llm::InferenceMessage::Metrics { decode_tok_s, .. } => {
                                let _ = tx_clone.send(StreamChunk::Token(format!(
                                    "\n\n⚡ [Saf Rust GGUF Motoru: {:.1} token/sn]",
                                    decode_tok_s
                                )));
                            }
                            corten_llm::InferenceMessage::Finished => break,
                            corten_llm::InferenceMessage::Error(e) => {
                                let _ = tx_clone.send(StreamChunk::Error(e));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx_clone.send(StreamChunk::Error(format!(
                        "Yerel GGUF modeli yüklenemedi: {}",
                        e
                    )));
                }
            }
            let _ = tx_clone.send(StreamChunk::Done);
        })
        .await;
    }

    /// Indexes desktop files into persistent WAL/bin index and performs sub-millisecond HNSW semantic search via `corten-vector`
    pub fn semantic_desktop_search(raw_query: &str) -> String {
        let clean_query = raw_query
            .trim_start_matches("dosya ara:")
            .trim_start_matches("dosya ara")
            .trim_start_matches("belge bul:")
            .trim_start_matches("belge bul")
            .trim_start_matches("kod ara:")
            .trim_start_matches("kod ara")
            .trim_start_matches("vektör ara:")
            .trim_start_matches("vektör ara")
            .trim_start_matches("vektörel dosya ara")
            .trim();

        let search_term = if clean_query.is_empty() {
            "wayland compositor rust"
        } else {
            clean_query
        };

        let total_indexed = AuraWorkspaceRag::ensure_indexed(false);
        let results = AuraWorkspaceRag::retrieve_rag_context(search_term, 5, None);

        if results.is_empty() {
            return format!(
                "🔍 Corten Vektör Motoru (`corten-vector`):\n'{}' sorgusu için eşleşen belge bulunamadı.",
                search_term
            );
        }

        let mut lines = Vec::new();
        for (rank, res) in results.iter().enumerate() {
            let path = res
                .metadata
                .get("path")
                .cloned()
                .unwrap_or_else(|| "Bilinmeyen dosya".into());
            let name = res
                .metadata
                .get("name")
                .cloned()
                .unwrap_or_else(|| path.clone());
            lines.push(format!(
                "{}. 📄 **{}** (Benzerlik: %{:.1})\n   `{}`",
                rank + 1,
                name,
                (res.similarity.clamp(0.0, 1.0)) * 100.0,
                path
            ));
        }

        format!(
            "🧠 **Corten Saf Rust Vektör Arama Motoru (`corten-vector` HNSW + WAL Kalıcı İndeks):**\n\
            Sorgu: *\"{}\"* ({} kalıcı belge indeksi)\n\n{}",
            search_term,
            total_indexed,
            lines.join("\n\n")
        )
    }

    async fn stream_ollama(
        host: &str,
        port: u16,
        model: &str,
        history: &[ChatMessage],
        prompt: &str,
        image_base64: Option<String>,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let addr = format!("{}:{}", host, port);
        let stream = match timeout(Duration::from_millis(1500), TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => s,
            _ => {
                let _ = tx.send(StreamChunk::Error("Ollama bağlantısı kurulamadı.".into()));
                let _ = tx.send(StreamChunk::Done);
                return;
            }
        };

        // Build messages payload for /api/chat
        let mut messages = Vec::new();
        let memory_ctx = AuraMemoryEngine::recall_context_for_prompt(prompt, 3);
        let relevant_tools = AuraToolRegistry::get_relevant_tools(prompt, 6);
        let tools_prompt = AuraToolRegistry::format_tools_prompt(&relevant_tools);
        // System prompt establishing Corten Desktop identity and action protocol with dynamic tool pruning
        let sys_prompt = format!(
            "Sen Corten Desktop sistem ve masaüstü otomasyon asistanısın (AURA-CORE). Kısa, teknik ve Türkçe yanıt ver.\n[KULLANICI HAFIZASI]:\n{}\n[KULLANILABİLİR ARAÇLAR]:\n{}\nKullanıcı sistem işlemi istediğinde cevabının sonuna gerekiyorsa [ACTION: intent] ekle.",
            memory_ctx, tools_prompt
        );
        messages.push(serde_json::json!({
            "role": "system",
            "content": sys_prompt
        }));

        for h in history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            let role_str = match h.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            };
            messages.push(serde_json::json!({
                "role": role_str,
                "content": h.content
            }));
        }

        let mut user_msg = serde_json::json!({
            "role": "user",
            "content": prompt
        });

        if let Some(img) = image_base64 {
            user_msg["images"] = serde_json::json!([img]);
        }
        messages.push(user_msg);

        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "options": {
                "temperature": 0.4
            }
        })
        .to_string();

        let req = format!(
            "POST /api/chat HTTP/1.1\r\n\
            Host: {}:{}\r\n\
            User-Agent: CortenDesktop-AI/0.2.0\r\n\
            Content-Type: application/json\r\n\
            Content-Length: {}\r\n\
            Connection: close\r\n\
            \r\n\
            {}",
            host,
            port,
            body.len(),
            body
        );

        let mut stream = stream;
        if stream.write_all(req.as_bytes()).await.is_err() {
            let _ = tx.send(StreamChunk::Error("İstek gönderilemedi.".into()));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        let mut in_headers = true;
        let mut filter = ThoughtStreamFilter::new();

        while let Ok(Some(line)) = lines.next_line().await {
            if in_headers {
                if line.trim().is_empty() {
                    in_headers = false;
                }
                continue;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Skip hex chunk-length lines in HTTP/1.1 chunked transfer
            if trimmed.chars().all(|c| c.is_ascii_hexdigit()) && trimmed.len() <= 8 {
                continue;
            }

            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(token) = val
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !token.is_empty() {
                        filter.process(token, &tx);
                    }
                }
                if let Some(done) = val.get("done").and_then(|d| d.as_bool()) {
                    if done {
                        break;
                    }
                }
            }
        }

        let _ = tx.send(StreamChunk::Done);
    }

    async fn stream_llama_cpp(
        host: &str,
        port: u16,
        model: &str,
        history: &[ChatMessage],
        prompt: &str,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let addr = format!("{}:{}", host, port);
        let stream = match timeout(Duration::from_millis(1500), TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => s,
            _ => {
                let _ = tx.send(StreamChunk::Error("llama-server bağlantısı kurulamadı.".into()));
                let _ = tx.send(StreamChunk::Done);
                return;
            }
        };

        let mut messages = Vec::new();
        let memory_ctx = AuraMemoryEngine::recall_context_for_prompt(prompt, 3);
        let relevant_tools = AuraToolRegistry::get_relevant_tools(prompt, 6);
        let tools_prompt = AuraToolRegistry::format_tools_prompt(&relevant_tools);
        let sys_prompt = format!(
            "Sen Corten Desktop sistem ve masaüstü otomasyon asistanısın (AURA-CORE). Türkçe, teknik ve doğrudan eylem odaklı yanıt ver.\n[KULLANICI HAFIZASI]:\n{}\n[KULLANILABİLİR ARAÇLAR]:\n{}",
            memory_ctx, tools_prompt
        );
        messages.push(serde_json::json!({
            "role": "system",
            "content": sys_prompt
        }));

        for h in history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            let role_str = match h.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            };
            messages.push(serde_json::json!({
                "role": role_str,
                "content": h.content
            }));
        }

        messages.push(serde_json::json!({
            "role": "user",
            "content": prompt
        }));

        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "max_tokens": 512,
            "temperature": 0.3
        })
        .to_string();

        let req = format!(
            "POST /v1/chat/completions HTTP/1.1\r\n\
            Host: {}:{}\r\n\
            User-Agent: CortenDesktop-AI/0.2.0\r\n\
            Content-Type: application/json\r\n\
            Content-Length: {}\r\n\
            Connection: close\r\n\
            \r\n\
            {}",
            host,
            port,
            body.len(),
            body
        );

        let mut stream = stream;
        if stream.write_all(req.as_bytes()).await.is_err() {
            let _ = tx.send(StreamChunk::Error("İstek gönderilemedi.".into()));
            let _ = tx.send(StreamChunk::Done);
            return;
        }

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();
        let mut in_headers = true;
        let mut filter = ThoughtStreamFilter::new();

        while let Ok(Some(line)) = lines.next_line().await {
            if in_headers {
                if line.trim().is_empty() {
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

            if let Some(json_str) = trimmed.strip_prefix("data: ") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let Some(token) = val
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|choice| choice.get("delta"))
                        .and_then(|delta| delta.get("content"))
                        .and_then(|content| content.as_str())
                    {
                        if !token.is_empty() {
                            filter.process(token, &tx);
                        }
                    }
                }
            }
        }

        let _ = tx.send(StreamChunk::Done);
    }

    async fn stream_aura_core(
        host: &str,
        port: u16,
        prompt: &str,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let addr = format!("{}:{}", host, port);

        // Try 1: Stream via AURA-CORE SSE endpoint (/agent/chat/stream)
        if let Ok(mut stream) = TcpStream::connect(&addr).await {
            let body = serde_json::json!({
                "query": prompt,
                "max_tokens": 512
            })
            .to_string();

            let req = format!(
                "POST /agent/chat/stream HTTP/1.1\r\n\
                Host: {}:{}\r\n\
                User-Agent: CortenDesktop-AI/0.2.0\r\n\
                Content-Type: application/json\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n\
                {}",
                host, port, body.len(), body
            );

            if stream.write_all(req.as_bytes()).await.is_ok() {
                let reader = BufReader::new(stream);
                let mut lines = reader.lines();
                let mut in_headers = true;
                let mut got_stream_token = false;
                let mut filter = ThoughtStreamFilter::new();

                while let Ok(Some(line)) = lines.next_line().await {
                    if in_headers {
                        if line.trim().is_empty() {
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

                    if let Some(json_str) = trimmed.strip_prefix("data: ") {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                            if let Some(token) = val.get("token").and_then(|t| t.as_str()) {
                                if !token.is_empty() {
                                    got_stream_token = true;
                                    filter.process(token, &tx);
                                }
                            }
                        }
                    }
                }

                if got_stream_token {
                    let _ = tx.send(StreamChunk::Done);
                    return;
                }
            }
        }

        // Try 2: AURA-CORE ReAct Agent with Tool Execution (/agent/chat)
        if let Ok(mut stream) = TcpStream::connect(&addr).await {
            let body = serde_json::json!({
                "query": prompt,
                "max_tokens": 512
            })
            .to_string();

            let req = format!(
                "POST /agent/chat HTTP/1.1\r\n\
                Host: {}:{}\r\n\
                User-Agent: CortenDesktop-AI/0.2.0\r\n\
                Content-Type: application/json\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n\
                {}",
                host, port, body.len(), body
            );

            if stream.write_all(req.as_bytes()).await.is_ok() {
                let mut buffer = Vec::new();
                if timeout(Duration::from_secs(15), stream.read_to_end(&mut buffer)).await.is_ok() {
                    let raw = String::from_utf8_lossy(&buffer);
                    if let Some(pos) = raw.find("\r\n\r\n") {
                        let body_json = &raw[pos + 4..];
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(body_json) {
                            // Format tool execution if used
                            if let Some(tool) = val.get("tool_used").and_then(|t| t.as_object()) {
                                let tool_name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                                let tool_res = val.get("tool_result")
                                    .map(|r| r.to_string().trim_matches('"').to_string())
                                    .unwrap_or_default();
                                let tool_msg = format!("🛠️ AURA-CORE Ajan Eylemi: `{}`\n📊 Çıktı: {}\n\n", tool_name, tool_res);
                                let _ = tx.send(StreamChunk::Token(tool_msg));
                            }
                            if let Some(ans) = val.get("answer").and_then(|a| a.as_str()) {
                                let _ = tx.send(StreamChunk::Token(ans.to_string()));
                                let _ = tx.send(StreamChunk::Done);
                                return;
                            }
                        }
                    }
                }
            }
        }

        // Try 3: Standard fallback (/api/generate)
        if let Ok(mut stream) = TcpStream::connect(&addr).await {
            let body = serde_json::json!({
                "prompt": prompt,
                "max_tokens": 512,
                "temperature": 0.3
            })
            .to_string();

            let req = format!(
                "POST /api/generate HTTP/1.1\r\n\
                Host: {}:{}\r\n\
                User-Agent: CortenDesktop-AI/0.2.0\r\n\
                Content-Type: application/json\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n\
                {}",
                host, port, body.len(), body
            );

            if stream.write_all(req.as_bytes()).await.is_ok() {
                let mut buffer = Vec::new();
                if timeout(Duration::from_secs(12), stream.read_to_end(&mut buffer)).await.is_ok() {
                    let raw = String::from_utf8_lossy(&buffer);
                    if let Some(pos) = raw.find("\r\n\r\n") {
                        let body = &raw[pos + 4..];
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
                            if let Some(text) = val
                                .get("response")
                                .or_else(|| val.get("text"))
                                .or_else(|| val.get("output"))
                                .and_then(|t| t.as_str())
                            {
                                let _ = tx.send(StreamChunk::Token(text.to_string()));
                            } else {
                                let _ = tx.send(StreamChunk::Token(body.trim().to_string()));
                            }
                        } else {
                            let _ = tx.send(StreamChunk::Token(body.trim().to_string()));
                        }
                    }
                }
            }
        }

        let _ = tx.send(StreamChunk::Done);
    }

    /// Captures the full screen using corten-snipper or screencopy and encodes to base64
    pub fn capture_screen_base64() -> Option<String> {
        let tmp_path = "/tmp/corten_ai_screen.png";
        let _ = std::fs::remove_file(tmp_path);

        // Try corten-snipper fullscreen capture
        let snipper = crate::actions::resolve_corten_bin("corten-snipper");
        if snipper.exists() {
            let status = std::process::Command::new(snipper)
                .args(["--fullscreen", "--output", tmp_path])
                .status()
                .ok()?;
            if status.success() {
                if let Ok(bytes) = std::fs::read(tmp_path) {
                    return Some(base64_encode(&bytes));
                }
            }
        }

        // Fallback: grim or slurp if available
        if let Ok(status) = std::process::Command::new("grim").arg(tmp_path).status() {
            if status.success() {
                if let Ok(bytes) = std::fs::read(tmp_path) {
                    return Some(base64_encode(&bytes));
                }
            }
        }

        None
    }

    /// Offline heuristic static security auditor for scripts and code
    pub fn heuristic_security_audit(query: &str) -> String {
        let lower = query.to_lowercase();
        let mut findings = Vec::new();
        let mut risk_score = 0;

        if lower.contains("curl ")
            || lower.contains("wget ")
            || lower.contains("http://")
            || lower.contains("https://")
        {
            findings.push("⚠️ Ağ İletişimi: İnternet üzerinden dış sunuculara HTTP/ağ isteği gönderiyor.");
            risk_score += 1;
        }
        if lower.contains(".ssh")
            || lower.contains("id_rsa")
            || lower.contains("authorized_keys")
        {
            findings.push("🚨 Hassas Dizin: SSH anahtarlarına erişim teşebbüsü tespit edildi!");
            risk_score += 3;
        }
        if lower.contains("sudo ")
            || lower.contains("pkexec ")
            || lower.contains("doas ")
        {
            findings.push("⚠️ Yetki Yükseltme: Root / yönetici hakları talep ediyor.");
            risk_score += 2;
        }
        if lower.contains("base64")
            && (lower.contains("-d") || lower.contains("--decode"))
        {
            findings.push("🚨 Gizli Yük: Base64 ile gizlenmiş (obfuscated) kod yürütme tespit edildi!");
            risk_score += 3;
        }
        if lower.contains("systemd/user")
            || lower.contains(".bashrc")
            || lower.contains(".profile")
        {
            findings.push("⚠️ Kalıcılık (Persistence): Oturum başlangıç dosyalarına kalıcılık eklemeye çalışıyor.");
            risk_score += 2;
        }
        if lower.contains("rm -rf")
            || lower.contains("mkfs")
            || lower.contains("dd if=")
        {
            findings.push("🚨 Yıkıcı Eylem: Dosya sistemi silme veya disk yazma komutu!");
            risk_score += 4;
        }

        let verdict = if risk_score >= 3 {
            "⛔ YÜKSEK RİSK: Bu betik şüpheli/zararlı komutlar içeriyor! Çalıştırmanız önerilmez."
        } else if risk_score > 0 {
            "⚠️ ORTA RİSK: Dış ağ istekleri veya yetki talepleri içeriyor. İnceleyerek çalıştırın."
        } else {
            "✅ DÜŞÜK RİSK / TEMİZ: Herhangi bir belirgin saldırı veya zararlı kod örüntüsü tespit edilmedi."
        };

        let details = if findings.is_empty() {
            "• Bilinen saldırı imzaları, arka kapılar veya gizli ters kabuk tespit edilmedi.\n• Standart dosya ve işlem yürütme akışı."
                .to_string()
        } else {
            findings.join("\n")
        };

        format!(
            "🛡️ Corten AI Güvenlik Raporu (Statik / Heuristik Sentinel):\n\n\
            {}\n\n\
            Tespit Edilen Unsurlar:\n{}\n\n\
            (Not: Bu değerlendirme Corten AI yerel güvenlik kalkanı tarafından çevrimdışı kurallarla üretilmiştir.)",
            verdict, details
        )
    }
}

/// Pure Rust RFC-compliant Base64 encoder (zero external dependencies)
pub fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).map(|&b| b as u32).unwrap_or(0);
        let b2 = chunk.get(2).map(|&b| b as u32).unwrap_or(0);
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARSET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARSET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARSET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARSET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Corten AI"), "Q29ydGVuIEFJ");
    }

    #[test]
    fn test_classify_model_tier() {
        assert_eq!(AiEngine::classify_model_tier("qwen2.5:1.5b"), "Eco (1.5B)");
        assert_eq!(AiEngine::classify_model_tier("llama3.2:1b"), "Eco (1.5B)");
        assert_eq!(AiEngine::classify_model_tier("qwen2.5:3b"), "Dengeli (3B)");
        assert_eq!(AiEngine::classify_model_tier("phi3:mini"), "Eco (1.5B)");
        assert_eq!(AiEngine::classify_model_tier("qwen2.5:7b"), "Pro (7B)");
        assert_eq!(AiEngine::classify_model_tier("deepseek-r1:8b"), "Pro (7B)");
    }

    #[test]
    fn test_thought_stream_filter_isolates_reasoning() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut filter = ThoughtStreamFilter::new();

        filter.process("Ön bilgi ", &tx);
        filter.process("<think>Düşünce süreci burada", &tx);
        filter.process(" ve devam ediyor</think>Nihai cevap metni.", &tx);

        let mut tokens = Vec::new();
        let mut thoughts = Vec::new();

        while let Ok(chunk) = rx.try_recv() {
            match chunk {
                StreamChunk::Token(t) => tokens.push(t),
                StreamChunk::Thought(t) => thoughts.push(t),
                _ => {}
            }
        }

        assert_eq!(tokens.join(""), "Ön bilgi Nihai cevap metni.");
        assert_eq!(thoughts.join(""), "Düşünce süreci burada ve devam ediyor");
    }

    #[test]
    fn test_offline_assistant_response_has_guidance() {
        let res = AiEngine::offline_assistant_response("merhaba");
        assert!(res.contains("Corten AI Sistem Asistanı"));
        assert!(res.contains("Ses & Parlaklık"));
        assert!(res.contains("Dosya Arama"));
    }
}
