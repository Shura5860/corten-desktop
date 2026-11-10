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
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Token(String),
    ActionExecuted(String),
    Done,
    Error(String),
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

impl AiEngine {
    /// Detect which local AI backend is running on loopback or embedded GGUF
    pub async fn auto_detect() -> BackendType {
        // 1. Check in-process Native Rust GGUF models first (0 external dependency, 100% pure Rust)
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

        // 2. Probe Ollama (port 11434)
        if let Ok(info) = Self::probe_ollama("127.0.0.1", 11434).await {
            return info;
        }

        // 3. Probe llama.cpp / LocalAI / OpenAI-compatible (port 8080)
        if let Ok(info) = Self::probe_llama_cpp("127.0.0.1", 8080).await {
            return info;
        }

        // 4. Probe AURA-CORE (port 8000)
        if let Ok(info) = Self::probe_aura_core("127.0.0.1", 8000).await {
            return info;
        }

        BackendType::Offline
    }

    async fn probe_ollama(host: &str, port: u16) -> Result<BackendType, ()> {
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
                        let active = names[0].clone();
                        info!("Corten AI: Found Ollama backend with model '{}'", active);
                        return Ok(BackendType::Ollama {
                            host: host.to_string(),
                            port,
                            model: active,
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
                // In offline mode, run the Pure-Rust Autonomous ReAct + Memory + RAG synthesizer!
                let react_report = AuraReActAgent::execute_react_workflow(&trimmed);
                let _ = tx.send(StreamChunk::Token(react_report));
                let _ = tx.send(StreamChunk::Done);
            }
        }
    }

    async fn stream_native_gguf(
        model_path: &std::path::Path,
        prompt: &str,
        tx: UnboundedSender<StreamChunk>,
    ) {
        let path_buf = model_path.to_path_buf();
        let memory_ctx = AuraMemoryEngine::recall_context_for_prompt(prompt, 3);
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
            "<|im_start|>system\nSen Corten Desktop yerel yapay zeka asistanısın. Türkçe ve net yanıt ver.\n[KULLANICI HAFIZASI]:\n{}{}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            memory_ctx, rag_ctx, prompt
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
        // System prompt establishing Corten Desktop identity and action protocol
        messages.push(serde_json::json!({
            "role": "system",
            "content": "Sen Corten Desktop için çalışan yüksek zekalı yerel AI asistanısın. Kısa, teknik ve Türkçe yanıt ver. Kullanıcı sistem işlemi istediğinde cevabının sonuna gerekiyorsa [ACTION: intent] ekle."
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

            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(token) = val
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !token.is_empty() {
                        let _ = tx.send(StreamChunk::Token(token.to_string()));
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
        messages.push(serde_json::json!({
            "role": "system",
            "content": "Sen Corten Desktop yerel asistanısın. Türkçe, teknik ve net yanıt ver."
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
                            let _ = tx.send(StreamChunk::Token(token.to_string()));
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

                    if let Some(json_str) = trimmed.strip_prefix("data: ") {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                            if let Some(token) = val.get("token").and_then(|t| t.as_str()) {
                                if !token.is_empty() {
                                    got_stream_token = true;
                                    let _ = tx.send(StreamChunk::Token(token.to_string()));
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
}
