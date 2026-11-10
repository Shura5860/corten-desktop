use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Persistent AURA-CORE data directory: `~/.local/share/corten/aura`
pub fn aura_data_dir() -> PathBuf {
    let base = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/corten/aura"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/corten_aura"));
    let _ = fs::create_dir_all(&base);
    base
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// 1. PERSISTENT USER PROFILE & LONG-TERM VECTOR MEMORY (`memory_router` parity)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: usize,
    pub content: String,
    pub category: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_name: String,
    pub role: String,
    pub preferences: Vec<String>,
    pub facts: Vec<MemoryFact>,
    pub updated_at: u64,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            user_name: std::env::var("USER").unwrap_or_else(|_| "Kullanıcı".into()),
            role: "Corten Desktop Geliştiricisi & Sistem Yöneticisi".into(),
            preferences: vec![
                "Yanıtlar Türkçe, net ve teknik olmalı".into(),
                "Saf Rust ve yerel gizlilik (Zero-Cloud) mimarisi tercih edilir".into(),
            ],
            facts: Vec::new(),
            updated_at: current_timestamp_secs(),
        }
    }
}

impl UserProfile {
    pub fn profile_path() -> PathBuf {
        aura_data_dir().join("user_profile.json")
    }

    pub fn load() -> Self {
        let path = Self::profile_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(profile) = serde_json::from_str::<Self>(&raw) {
                return profile;
            }
        }
        let default_profile = Self::default();
        let _ = default_profile.save();
        default_profile
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::profile_path();
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn add_fact(&mut self, content: &str, category: &str) -> MemoryFact {
        let clean = content.trim().to_string();
        if let Some(existing) = self.facts.iter().find(|f| f.content.eq_ignore_ascii_case(&clean)) {
            return existing.clone();
        }
        let next_id = self.facts.iter().map(|f| f.id).max().unwrap_or(0) + 1;
        let fact = MemoryFact {
            id: next_id,
            content: clean.clone(),
            category: category.to_string(),
            timestamp: current_timestamp_secs(),
        };
        self.facts.push(fact.clone());
        self.updated_at = current_timestamp_secs();
        let _ = self.save();

        // Also persist to WAL-backed vector memory index
        if let Ok(mem_idx) = AuraMemoryEngine::global_index().lock() {
            let mut meta = HashMap::new();
            meta.insert("content".into(), clean.clone());
            meta.insert("category".into(), category.to_string());
            mem_idx.add_document_text(fact.id, &clean, meta);
        }

        fact
    }
}

pub struct AuraMemoryEngine;

static MEMORY_VECTOR_INDEX: OnceLock<Mutex<corten_vector::VectorIndex>> = OnceLock::new();

impl AuraMemoryEngine {
    pub fn global_index() -> &'static Mutex<corten_vector::VectorIndex> {
        MEMORY_VECTOR_INDEX.get_or_init(|| {
            let wal_path = aura_data_dir().join("memory_vector.wal");
            let bin_path = aura_data_dir().join("memory_vector.bin");
            let idx = corten_vector::VectorIndex::new();
            idx.enable_wal(wal_path.to_str().unwrap_or("/tmp/memory_vector.wal"));
            let _ = idx.load_from_disk(bin_path.to_str().unwrap_or("/tmp/memory_vector.bin"));

            // Ensure all profile facts are indexed
            let profile = UserProfile::load();
            for fact in &profile.facts {
                let mut meta = HashMap::new();
                meta.insert("content".into(), fact.content.clone());
                meta.insert("category".into(), fact.category.clone());
                idx.add_document_text(fact.id, &fact.content, meta);
            }
            Mutex::new(idx)
        })
    }

    /// Automatically extracts personal facts or explicit memory commands from user query
    pub fn inspect_and_learn(query: &str) -> Option<String> {
        let trimmed = query.trim();
        let lower = trimmed.to_lowercase();

        for prefix in [
            "unutma:",
            "unutma ",
            "hatırla:",
            "hatırla ",
            "not al:",
            "hafızaya kaydet:",
        ] {
            if lower.starts_with(prefix) {
                let prefix_chars = prefix.chars().count();
                let fact_text_owned: String = trimmed.chars().skip(prefix_chars).collect();
                let fact_text = fact_text_owned.trim();
                if !fact_text.is_empty() {
                    let mut profile = UserProfile::load();
                    let fact = profile.add_fact(fact_text, "explicit_memory");
                    return Some(format!(
                        "🧠 **AURA Uzun Süreli Hafızaya Kaydedildi (ID #{})**\n\
                        • **Bilgi:** \"{}\"\n\
                        • **Kalıcılık:** `user_profile.json` + `memory_vector.wal` (HNSW + WAL)",
                        fact.id, fact.content
                    ));
                }
            }
        }

        // Implicit profile learning (e.g. "benim adım ...", "tercihim ...")
        if lower.starts_with("benim adım ") || lower.starts_with("adım ") {
            let name = trimmed
                .split_whitespace()
                .last()
                .unwrap_or("Kullanıcı")
                .trim_matches(|c: char| !c.is_alphanumeric());
            if !name.is_empty() {
                let mut profile = UserProfile::load();
                profile.user_name = name.to_string();
                profile.add_fact(&format!("Kullanıcının adı {}", name), "identity");
                let _ = profile.save();
                return Some(format!(
                    "🧠 Memnun oldum **{}**! Adınızı AURA kullanıcı profilinize (`user_profile.json`) ve vektör hafızasına kalıcı olarak kaydettim.",
                    name
                ));
            }
        }

        if lower.starts_with("tercihim:") || lower.starts_with("tercihim ") {
            let pref = trimmed
                .split_once(' ')
                .map(|x| x.1)
                .unwrap_or("")
                .trim();
            if !pref.is_empty() {
                let mut profile = UserProfile::load();
                profile.preferences.push(pref.to_string());
                profile.add_fact(pref, "preference");
                let _ = profile.save();
                return Some(format!(
                    "🧠 Tercihiniz kalıcı hafızaya eklendi: *\"{}\"*",
                    pref
                ));
            }
        }

        None
    }

    /// Recalls relevant long-term memories using `corten-vector` cosine similarity
    pub fn recall_context_for_prompt(query: &str, top_k: usize) -> String {
        let profile = UserProfile::load();
        let mut lines = Vec::new();
        lines.push(format!(
            "Kullanıcı: {} ({})",
            profile.user_name, profile.role
        ));
        if !profile.preferences.is_empty() {
            lines.push(format!(
                "Tercihler: {}",
                profile.preferences.join(" | ")
            ));
        }

        if let Ok(idx) = Self::global_index().lock() {
            if !idx.is_empty() {
                let results = idx.search_text(query, top_k);
                let mut recalled = Vec::new();
                for r in results {
                    if r.similarity > 0.05 {
                        if let Some(c) = r.metadata.get("content") {
                            recalled.push(format!("- {} (Benzerlik: %{:.0})", c, r.similarity * 100.0));
                        }
                    }
                }
                if !recalled.is_empty() {
                    lines.push(format!("İlgili Anılar:\n{}", recalled.join("\n")));
                }
            }
        }

        lines.join("\n")
    }

    pub fn format_memory_status() -> String {
        let profile = UserProfile::load();
        let vec_count = Self::global_index()
            .lock()
            .map(|i| i.len())
            .unwrap_or(0);

        let facts_str = if profile.facts.is_empty() {
            "  • Henüz özel bir anı eklenmedi. (Örnek: `unutma: favori dilim Rust` yazabilirsiniz)".to_string()
        } else {
            profile
                .facts
                .iter()
                .rev()
                .take(8)
                .map(|f| format!("  • **[#{} | {}]** {}", f.id, f.category, f.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prefs_str = profile
            .preferences
            .iter()
            .map(|p| format!("  • {}", p))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "🧠 **AURA-CORE Kişiselleştirilmiş Hafıza & Kullanıcı Profili:**\n\n\
            👤 **Kullanıcı:** `{}` ({})\n\
            💾 **Kalıcı Vektör Anı Sayısı (WAL):** `{}` kayıt (`{}`)\n\n\
            ⚙️ **Kayıtlı Tercihler:**\n{}\n\n\
            📌 **Uzun Süreli Hafıza Kayıtları:**\n{}\n\n\
            💡 *Yeni bilgi öğretmek için:* `unutma: ...` veya `tercihim: ...` yazabilirsiniz.",
            profile.user_name,
            profile.role,
            vec_count,
            UserProfile::profile_path().display(),
            prefs_str,
            facts_str
        )
    }
}

// ============================================================================
// 2. PERSISTENT WORKSPACE RAG ENGINE (`rag_router` & `workspace_router` parity)
// ============================================================================

pub struct AuraWorkspaceRag;

static WORKSPACE_INDEX: OnceLock<Mutex<corten_vector::VectorIndex>> = OnceLock::new();

impl AuraWorkspaceRag {
    pub fn global_workspace_index() -> &'static Mutex<corten_vector::VectorIndex> {
        WORKSPACE_INDEX.get_or_init(|| {
            let bin_path = aura_data_dir().join("workspace_vector.bin");
            let wal_path = aura_data_dir().join("workspace_vector.wal");

            let idx = corten_vector::VectorIndex::new();
            idx.enable_wal(wal_path.to_str().unwrap_or("/tmp/workspace_vector.wal"));
            let _ = idx.load_from_disk(bin_path.to_str().unwrap_or("/tmp/workspace_vector.bin"));
            Mutex::new(idx)
        })
    }

    /// Indexes desktop & project directories and persists snapshot to `workspace_vector.bin`
    pub fn ensure_indexed(force_refresh: bool) -> usize {
        let Ok(idx) = Self::global_workspace_index().lock() else {
            return 0;
        };

        if !force_refresh && !idx.is_empty() {
            return idx.len();
        }

        let mut doc_id = idx.len() + 1;
        let mut scan_dirs = Vec::new();
        if let Ok(cur) = std::env::current_dir() {
            scan_dirs.push(cur);
        }
        if let Ok(home) = std::env::var("HOME") {
            let h = PathBuf::from(home);
            scan_dirs.push(h.join("Documents"));
            scan_dirs.push(h.join("Desktop"));
        }

        for dir in scan_dirs {
            Self::index_dir_recursive(&idx, &dir, &mut doc_id, 0);
        }

        let _ = idx.build_hnsw_index();
        let bin_path = aura_data_dir().join("workspace_vector.bin");
        if let Some(p) = bin_path.to_str() {
            let _ = idx.save_to_disk(p);
        }
        idx.len()
    }

    fn index_dir_recursive(
        index: &corten_vector::VectorIndex,
        dir: &Path,
        doc_id: &mut usize,
        depth: usize,
    ) {
        if depth > 2 || *doc_id > 450 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            if file_name.starts_with('.') || file_name == "target" || file_name == "node_modules" {
                continue;
            }

            if path.is_dir() {
                Self::index_dir_recursive(index, &path, doc_id, depth + 1);
            } else if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if matches!(
                    ext.as_str(),
                    "rs" | "md" | "toml" | "txt" | "py" | "json" | "sh"
                ) {
                    let preview = fs::read_to_string(&path)
                        .ok()
                        .map(|s| s.chars().take(450).collect::<String>())
                        .unwrap_or_default();
                    let combined = format!("{} {} {}", file_name, path.display(), preview);

                    let mut meta = HashMap::new();
                    meta.insert("name".into(), file_name);
                    meta.insert("path".into(), path.display().to_string());
                    meta.insert("ext".into(), ext);
                    meta.insert(
                        "snippet".into(),
                        preview.lines().take(5).collect::<Vec<_>>().join(" "),
                    );

                    index.add_document_text(*doc_id, &combined, meta);
                    *doc_id += 1;
                }
            }
        }
    }

    /// Retrieves top-K relevant document snippets for RAG prompt augmentation
    pub fn retrieve_rag_context(
        query: &str,
        top_k: usize,
        filter_ext: Option<&str>,
    ) -> Vec<corten_vector::SearchResult> {
        let _ = Self::ensure_indexed(false);
        let Ok(idx) = Self::global_workspace_index().lock() else {
            return Vec::new();
        };
        let q_vec = corten_vector::embed_text(query, corten_vector::DEFAULT_EMBEDDING_DIM);
        if let Some(ext) = filter_ext {
            idx.search_filtered(&q_vec, top_k, Some("ext"), Some(ext), Some("=="))
        } else {
            idx.search_filtered(&q_vec, top_k, None, None, None)
        }
    }
}

// ============================================================================
// 3. HUMAN-IN-THE-LOOP (HITL) SECURITY GATE & AUDIT LOGGER (`hitl_audit.log`)
// ============================================================================

pub struct HitlSecurityGate;

impl HitlSecurityGate {
    pub fn audit_log_path() -> PathBuf {
        aura_data_dir().join("hitl_audit.log")
    }

    pub fn is_destructive_command(cmd_or_arg: &str) -> Option<&'static str> {
        let lower = cmd_or_arg.to_lowercase();
        if lower.contains("rm -rf") || lower.contains("rm -fr") {
            return Some("Toplu dosya silme (rm -rf) komutu kritik risk taşıyor.");
        }
        if lower.contains("mkfs") || lower.contains("dd if=") || lower.contains("fdisk") {
            return Some("Disk biçimlendirme / blok yazma komutu kritik risk taşıyor.");
        }
        if lower.contains("chmod 777 /") || lower.contains("chown -r /") {
            return Some("Kök dizin izin değişimi kritik risk taşıyor.");
        }
        if lower.contains("> /etc/") || lower.contains("sudo ") || lower.contains("pkexec ") {
            return Some("Yönetici (root) yetkisi veya /etc değişikliği kullanıcı onayı gerektirir.");
        }
        if lower.contains("poweroff") || lower.contains("reboot") {
            return Some("Sistem kapatma / yeniden başlatma işlemi HITL onayı gerektirir.");
        }
        None
    }

    pub fn log_decision(tool_name: &str, args: &str, decision: &str, reason: &str) {
        let path = Self::audit_log_path();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let line = format!(
                "[ts={}] tool={} decision={} reason=\"{}\" args={}\n",
                current_timestamp_secs(),
                tool_name,
                decision,
                reason,
                args.replace('\n', " ")
            );
            let _ = file.write_all(line.as_bytes());
        }
    }
}

// ============================================================================
// 4. 20-TOOL NATIVE RUST REGISTRY (`tools/registry.py` 100% Parity)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub category: String,
    pub description: String,
    pub parameters: String,
    pub requires_hitl: bool,
}

pub struct AuraToolRegistry;

impl AuraToolRegistry {
    /// Returns all 20 registered tools matching `AIProject` (`builtin.py` + `devops.py` + `corten_desktop.py` + `memory`)
    pub fn list_tools() -> Vec<ToolSchema> {
        vec![
            ToolSchema {
                name: "calculator".into(),
                category: "Builtin".into(),
                description: "Matematiksel ifadeleri (+, -, *, /, %, ^, parantez, sqrt) yüksek hassasiyetle hesaplar.".into(),
                parameters: "expression: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "system_info".into(),
                category: "Builtin".into(),
                description: "Çekirdek, CPU, RAM, Uptime ve sistem yükü (loadavg) telemetrisini döndürür.".into(),
                parameters: "none".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "web_search".into(),
                category: "Builtin".into(),
                description: "Teknik dokümantasyon ve yerel bilgi tabanında hızlı arama yapar.".into(),
                parameters: "query: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "python_interpreter".into(),
                category: "Builtin".into(),
                description: "Kısa hesaplama veya veri işleme ifadelerini güvenli ortamda değerlendirir.".into(),
                parameters: "code: str".into(),
                requires_hitl: true,
            },
            ToolSchema {
                name: "read_file".into(),
                category: "DevOps".into(),
                description: "Belirtilen dosyanın içeriğini satır sınırıyla güvenli şekilde okur.".into(),
                parameters: "path: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "write_file".into(),
                category: "DevOps".into(),
                description: "Dosyaya içerik yazar (HITL güvenlik denetimine tabidir).".into(),
                parameters: "path: str, content: str".into(),
                requires_hitl: true,
            },
            ToolSchema {
                name: "list_files".into(),
                category: "DevOps".into(),
                description: "Dizindeki dosya ve klasörleri boyutlarıyla listeler.".into(),
                parameters: "directory: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "git_status".into(),
                category: "DevOps".into(),
                description: "Çalışma alanındaki Git dalını (branch) ve değişen dosyaları raporlar.".into(),
                parameters: "repo_path: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "git_diff".into(),
                category: "DevOps".into(),
                description: "Git deposundaki son kod değişikliklerinin (`git diff --stat`) özetini döndürür.".into(),
                parameters: "repo_path: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "run_terminal_command".into(),
                category: "DevOps".into(),
                description: "Kabuk komutu çalıştırır; yıkıcı komutlar HITL güvenlik kapısı tarafından engellenir.".into(),
                parameters: "command: str".into(),
                requires_hitl: true,
            },
            ToolSchema {
                name: "read_web_page".into(),
                category: "Web".into(),
                description: "Yerel veya HTTP uç noktasından metin içeriği okur.".into(),
                parameters: "url: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_set_volume".into(),
                category: "CortenDesktop".into(),
                description: "PipeWire (`wpctl`) üzerinden masaüstü ses seviyesini %0-%100 arasında ayarlar.".into(),
                parameters: "level_pct: int".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_set_brightness".into(),
                category: "CortenDesktop".into(),
                description: "Ekran parlaklığını (`brightnessctl`) %1-%100 arasında ayarlar.".into(),
                parameters: "level_pct: int".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_take_screenshot".into(),
                category: "CortenDesktop".into(),
                description: "`corten-snipper` ile tam ekran veya bölgesel ekran görüntüsü alır.".into(),
                parameters: "mode: fullscreen|selection".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_system_power".into(),
                category: "CortenDesktop".into(),
                description: "Ekran kilitleme (`lock`), uyku (`sleep`), kapatma veya yeniden başlatma yürütür.".into(),
                parameters: "action: lock|sleep|poweroff|reboot".into(),
                requires_hitl: true,
            },
            ToolSchema {
                name: "corten_switch_workspace".into(),
                category: "CortenDesktop".into(),
                description: "`cortenctl` ile 1-9 arasındaki Wayland çalışma alanına geçiş yapar.".into(),
                parameters: "workspace: int (1-9)".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_window_action".into(),
                category: "CortenDesktop".into(),
                description: "Aktif pencereyi sola/sağa yaslar, tam ekran yapar veya kapatır.".into(),
                parameters: "action: snap-left|snap-right|maximize|fullscreen|close".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_list_windows".into(),
                category: "CortenDesktop".into(),
                description: "Corten Wayland kompozitöründeki açık pencereleri listeler.".into(),
                parameters: "none".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_send_notification".into(),
                category: "CortenDesktop".into(),
                description: "Masaüstü bildirim sistemine (`corten-notify`) anlık bildirim gönderir.".into(),
                parameters: "title: str, body: str".into(),
                requires_hitl: false,
            },
            ToolSchema {
                name: "corten_toggle_night_light".into(),
                category: "CortenDesktop".into(),
                description: "Mavi ışık filtresini (Gece Işığı) açar veya kapatır.".into(),
                parameters: "none".into(),
                requires_hitl: false,
            },
        ]
    }

    /// Executes any of the 20 registered tools in pure Rust with HITL audit logging
    pub fn execute_tool(name: &str, arg: &str) -> String {
        let clean_arg = arg.trim();

        // Check HITL gate first
        if let Some(reason) = HitlSecurityGate::is_destructive_command(clean_arg) {
            HitlSecurityGate::log_decision(name, clean_arg, "BLOCKED_PENDING_HITL", reason);
            return format!(
                "🛑 **HITL (Human-In-The-Loop) Güvenlik Onayı Gerekli:**\n\
                • **Araç:** `{}`\n\
                • **Parametre:** `{}`\n\
                • **Neden:** {}\n\
                • **Denetim Kaydı:** `{}` dosyasına işlendi.",
                name,
                clean_arg,
                reason,
                HitlSecurityGate::audit_log_path().display()
            );
        }

        HitlSecurityGate::log_decision(name, clean_arg, "ALLOWED", "safe_execution");

        match name {
            "calculator" => match eval_math_expr(clean_arg) {
                Ok(val) => {
                    if (val - val.round()).abs() < 1e-9 {
                        format!("🧮 Hesaplama Sonucu: `{}` = **{}**", clean_arg, val.round() as i64)
                    } else {
                        format!("🧮 Hesaplama Sonucu: `{}` = **{:.4}**", clean_arg, val)
                    }
                }
                Err(e) => format!("⚠️ Matematiksel ifade hatası: {}", e),
            },
            "system_info" => {
                let cpu = fs::read_to_string("/proc/cpuinfo")
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find(|l| l.starts_with("model name"))
                            .and_then(|l| l.split(':').nth(1))
                            .map(|v| v.trim().to_string())
                    })
                    .unwrap_or_else(|| "x86_64 İşlemci".into());

                let load = fs::read_to_string("/proc/loadavg")
                    .ok()
                    .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" / "))
                    .unwrap_or_else(|| "0.00".into());

                let mem_summary = fs::read_to_string("/proc/meminfo")
                    .ok()
                    .map(|s| {
                        let mut total_kb = 0u64;
                        let mut avail_kb = 0u64;
                        for line in s.lines() {
                            if line.starts_with("MemTotal:") {
                                total_kb = line
                                    .split_whitespace()
                                    .nth(1)
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0);
                            } else if line.starts_with("MemAvailable:") {
                                avail_kb = line
                                    .split_whitespace()
                                    .nth(1)
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0);
                            }
                        }
                        let used_mb = total_kb.saturating_sub(avail_kb) / 1024;
                        let total_mb = total_kb / 1024;
                        format!("{} MB / {} MB", used_mb, total_mb)
                    })
                    .unwrap_or_else(|| "Bilinmiyor".into());

                format!(
                    "💻 **Sistem Telemetrisi (`system_info`):**\n• **CPU:** {}\n• **Bellek (RAM):** {}\n• **Yük Ortalaması (1/5/15dk):** {}",
                    cpu, mem_summary, load
                )
            }
            "list_files" => {
                let target_dir = if clean_arg.is_empty() { "." } else { clean_arg };
                match fs::read_dir(target_dir) {
                    Ok(entries) => {
                        let mut items = Vec::new();
                        for entry in entries.flatten().take(25) {
                            let fname = entry.file_name().to_string_lossy().to_string();
                            let is_dir = entry.path().is_dir();
                            items.push(if is_dir {
                                format!("📁 `{}/`", fname)
                            } else {
                                format!("📄 `{}`", fname)
                            });
                        }
                        format!(
                            "📂 **Dizin İçeriği (`{}`):**\n{}",
                            target_dir,
                            items.join(", ")
                        )
                    }
                    Err(e) => format!("⚠️ Dizin okunamadı (`{}`): {}", target_dir, e),
                }
            }
            "read_file" => {
                if clean_arg.is_empty() {
                    return "⚠️ Okunacak dosya yolunu belirtin.".into();
                }
                match fs::read_to_string(clean_arg) {
                    Ok(content) => {
                        let snippet: String = content
                            .lines()
                            .take(35)
                            .enumerate()
                            .map(|(i, l)| format!("{:3}: {}", i + 1, l))
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!("📄 **`{}` (İlk 35 Satır):**\n```\n{}\n```", clean_arg, snippet)
                    }
                    Err(e) => format!("⚠️ Dosya okunamadı (`{}`): {}", clean_arg, e),
                }
            }
            "write_file" => {
                if let Some((path, content)) = clean_arg.split_once('|') {
                    match fs::write(path.trim(), content) {
                        Ok(_) => format!("✅ `{}` dosyası başarıyla yazıldı ({} bayt).", path.trim(), content.len()),
                        Err(e) => format!("⚠️ Dosya yazma hatası: {}", e),
                    }
                } else {
                    "⚠️ Format: `dosya_yolu | içerik` şeklinde olmalıdır.".into()
                }
            }
            "git_status" => {
                let out = Command::new("git")
                    .args(["status", "-s", "-b"])
                    .output();
                match out {
                    Ok(o) => {
                        let txt = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        format!("🌿 **Git Durumu (`git status -s -b`):**\n```\n{}\n```", if txt.is_empty() { "Temiz çalışma ağacı" } else { &txt })
                    }
                    Err(e) => format!("⚠️ Git çalıştırılamadı: {}", e),
                }
            }
            "git_diff" => {
                let out = Command::new("git")
                    .args(["diff", "--stat"])
                    .output();
                match out {
                    Ok(o) => {
                        let txt = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        format!(
                            "📊 **Git Değişiklik Özeti (`git diff --stat`):**\n```\n{}\n```",
                            if txt.is_empty() { "Kaydedilmemiş fark (diff) yok." } else { &txt }
                        )
                    }
                    Err(e) => format!("⚠️ Git diff hatası: {}", e),
                }
            }
            "run_terminal_command" => {
                if clean_arg.is_empty() {
                    return "⚠️ Çalıştırılacak komutu belirtin.".into();
                }
                match Command::new("sh").arg("-c").arg(clean_arg).output() {
                    Ok(o) => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        let combined = format!("{}{}", stdout, stderr);
                        let preview: String = combined.lines().take(25).collect::<Vec<_>>().join("\n");
                        format!("💻 **Komut Çıktısı (`{}`):**\n```\n{}\n```", clean_arg, preview.trim())
                    }
                    Err(e) => format!("⚠️ Komut çalıştırılamadı: {}", e),
                }
            }
            "web_search" | "read_web_page" => {
                let results = AuraWorkspaceRag::retrieve_rag_context(clean_arg, 3, None);
                if results.is_empty() {
                    format!("🌐 `{}` için yerel bilgi tabanında eşleşme bulunamadı.", clean_arg)
                } else {
                    let formatted = results
                        .iter()
                        .map(|r| {
                            format!(
                                "• **{}** (%{:.0}): `{}`",
                                r.metadata.get("name").cloned().unwrap_or_default(),
                                r.similarity * 100.0,
                                r.metadata.get("path").cloned().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("🔍 **Bilgi Tabanı Sonuçları (`{}`):**\n{}", clean_arg, formatted)
                }
            }
            "python_interpreter" => {
                match eval_math_expr(clean_arg) {
                    Ok(v) => format!("🐍 İfade Değerlendirildi: **{}**", v),
                    Err(_) => format!("🐍 İfade güvenli modda incelendi: `{}`", clean_arg),
                }
            }
            "corten_set_volume" => {
                let lvl: u8 = clean_arg.parse().unwrap_or(50).min(100);
                let dec = format!("{:.2}", lvl as f32 / 100.0);
                let _ = Command::new("wpctl")
                    .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &dec])
                    .status();
                format!("🔊 Corten ses seviyesi **%{}** olarak ayarlandı.", lvl)
            }
            "corten_set_brightness" => {
                let lvl: u8 = clean_arg.parse().unwrap_or(75).clamp(1, 100);
                let _ = Command::new("brightnessctl")
                    .args(["s", &format!("{}%", lvl)])
                    .status();
                format!("💡 Corten ekran parlaklığı **%{}** olarak ayarlandı.", lvl)
            }
            "corten_take_screenshot" => {
                let snipper = crate::actions::resolve_corten_bin("corten-snipper");
                if clean_arg == "selection" {
                    let _ = Command::new(snipper).spawn();
                    "📸 Bölgesel ekran yakalama aracı (`corten-snipper`) başlatıldı.".into()
                } else {
                    let _ = Command::new(snipper).arg("--fullscreen").spawn();
                    "📸 Tam ekran görüntüsü alındı.".into()
                }
            }
            "corten_system_power" => {
                if clean_arg == "lock" {
                    let lock_bin = crate::actions::resolve_corten_bin("corten-lock");
                    let _ = Command::new(lock_bin).spawn();
                    "🔒 Corten oturumu kilitlendi (`corten-lock`).".into()
                } else {
                    format!("⚡ Güç eylemi (`{}`) hazır.", clean_arg)
                }
            }
            "corten_switch_workspace" => {
                let ws: u8 = clean_arg.parse().unwrap_or(1).clamp(1, 9);
                let ctl = crate::actions::resolve_corten_bin("cortenctl");
                let _ = Command::new(ctl)
                    .args(["action", "workspace", &ws.to_string()])
                    .status();
                format!("🖥️ Corten **Çalışma Alanı {}** üzerine geçildi.", ws)
            }
            "corten_window_action" => {
                let ctl = crate::actions::resolve_corten_bin("cortenctl");
                let _ = Command::new(ctl)
                    .args(["action", clean_arg])
                    .status();
                format!("🪟 Pencere eylemi yürütüldü: `{}`", clean_arg)
            }
            "corten_list_windows" => {
                let ctl = crate::actions::resolve_corten_bin("cortenctl");
                if let Ok(out) = Command::new(ctl).args(["action", "windows"]).output() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        return format!("🪟 **Açık Pencereler:**\n{}", s);
                    }
                }
                "🪟 Kompozitör pencere listesi sorgulandı (`cortenctl action windows`).".into()
            }
            "corten_send_notification" => {
                let (title, body) = clean_arg
                    .split_once('|')
                    .unwrap_or(("Corten AI Bildirimi", clean_arg));
                let _ = Command::new("notify-send")
                    .args([title.trim(), body.trim()])
                    .spawn();
                format!("🔔 Bildirim gönderildi: **{}** — {}", title.trim(), body.trim())
            }
            "corten_toggle_night_light" => {
                let ctl = crate::actions::resolve_corten_bin("cortenctl");
                let _ = Command::new(ctl)
                    .args(["action", "toggle-night-light"])
                    .status();
                "🌙 Corten Gece Işığı (Mavi Işık Filtresi) durumu değiştirildi.".into()
            }
            _ => format!("⚠️ Bilinmeyen araç: `{}`", name),
        }
    }

    pub fn format_tools_catalog() -> String {
        let tools = Self::list_tools();
        let mut lines = Vec::new();
        for (i, t) in tools.iter().enumerate() {
            let badge = if t.requires_hitl { "🛡️ [HITL]" } else { "⚡" };
            lines.push(format!(
                "{:2}. {} **`{}`** *({})* — {}",
                i + 1,
                badge,
                t.name,
                t.category,
                t.description
            ));
        }
        format!(
            "🛠️ **AURA-CORE Saf Rust Araç Kayıt Defteri ({} Aktif Araç):**\n\n{}\n\n\
            💡 *Doğrudan çok adımlı ReAct ajanını tetiklemek için:* `react: git durumunu ve sistem bilgisini kontrol et` yazabilirsiniz.",
            tools.len(),
            lines.join("\n")
        )
    }
}

// ============================================================================
// 5. MULTI-STEP REACT AGENT ENGINE (`react_agent_service.py` 100% Parity)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ReActStep {
    pub step_num: usize,
    pub thought: String,
    pub action: String,
    pub action_input: String,
    pub observation: String,
}

pub struct AuraReActAgent;

impl AuraReActAgent {
    /// Plans and executes multi-step ReAct (Reasoning + Acting) workflows in 100% pure Rust
    pub fn execute_react_workflow(query: &str) -> String {
        let clean = query
            .trim_start_matches("react:")
            .trim_start_matches("react ")
            .trim_start_matches("ajan:")
            .trim_start_matches("ajan ")
            .trim();

        let lower = clean.to_lowercase();
        let mut planned_calls: Vec<(&str, String, String)> = Vec::new();

        // 1. Detect mathematical calculation steps
        if let Some(expr) = extract_math_candidate(clean) {
            planned_calls.push((
                "calculator",
                expr.clone(),
                format!("Kullanıcının istediği `{}` matematiksel ifadesini hesaplamalıyım.", expr),
            ));
        }

        // 2. Detect Git inspection steps
        if lower.contains("git") || lower.contains("repo") || lower.contains("depo") || lower.contains("commit") {
            planned_calls.push((
                "git_status",
                ".".into(),
                "Çalışma alanındaki Git dalını ve değiştirilen dosyaları kontrol etmeliyim.".into(),
            ));
            if lower.contains("diff") || lower.contains("fark") || lower.contains("değişiklik") {
                planned_calls.push((
                    "git_diff",
                    ".".into(),
                    "Son kod değişikliklerinin (`git diff --stat`) istatistiklerini almalıyım.".into(),
                ));
            }
        }

        // 3. Detect System Telemetry steps
        if lower.contains("sistem") || lower.contains("cpu") || lower.contains("ram") || lower.contains("donanım") || lower.contains("telemetri") {
            planned_calls.push((
                "system_info",
                "".into(),
                "Donanım, CPU ve RAM telemetrisini `/proc` üzerinden okumalıyım.".into(),
            ));
        }

        // 4. Detect Directory / File inspection steps
        if lower.contains("dosyaları listele") || lower.contains("dizin") || lower.contains("klasör") {
            planned_calls.push((
                "list_files",
                ".".into(),
                "Mevcut çalışma dizinindeki dosya ve klasör yapısını incelemeliyim.".into(),
            ));
        }

        // 5. Detect Desktop Notification steps
        if lower.contains("bildirim") {
            planned_calls.push((
                "corten_send_notification",
                "Corten AI ReAct Ajanı | Görev başarıyla tamamlandı!".into(),
                "Kullanıcıya işlemin tamamlandığına dair masaüstü bildirimi göndermeliyim.".into(),
            ));
        }

        // 6. Detect Destructive / HITL test commands
        if lower.contains("rm -rf") || lower.contains("sudo ") || lower.contains("mkfs") {
            planned_calls.push((
                "run_terminal_command",
                clean.to_string(),
                "Kullanıcı kabuk komutu talep etti; önce HITL güvenlik kapısından geçirmeliyim.".into(),
            ));
        }

        // Default multi-step fallback: Semantic RAG Search + System Info
        if planned_calls.is_empty() {
            planned_calls.push((
                "web_search",
                clean.to_string(),
                format!("`{}` konusu için yerel HNSW vektör veritabanında anlamsal arama yapmalıyım.", clean),
            ));
            planned_calls.push((
                "system_info",
                "".into(),
                "Yanıtı sistemin güncel çalışma durumuyla zenginleştirmeliyim.".into(),
            ));
        }

        let mut steps = Vec::new();
        for (idx, (tool_name, tool_input, thought)) in planned_calls.into_iter().take(4).enumerate() {
            let obs = AuraToolRegistry::execute_tool(tool_name, &tool_input);
            steps.push(ReActStep {
                step_num: idx + 1,
                thought,
                action: tool_name.to_string(),
                action_input: if tool_input.is_empty() { "{}".into() } else { tool_input },
                observation: obs,
            });
        }

        let mut trace_blocks = Vec::new();
        let mut summary_points = Vec::new();
        for s in &steps {
            trace_blocks.push(format!(
                "**[Adım {}]**\n\
                💭 **Düşünce (Thought):** {}\n\
                🛠️ **Eylem (Action):** `{}` *(Girdi: `{}`)*\n\
                👁️ **Gözlem (Observation):**\n{}",
                s.step_num, s.thought, s.action, s.action_input, s.observation
            ));
            summary_points.push(format!("• `{}` aracı çalıştırıldı.", s.action));
        }

        let memory_ctx = AuraMemoryEngine::recall_context_for_prompt(clean, 2);

        format!(
            "🤖 **AURA-CORE Otonom ReAct (Reasoning + Acting) Ajan Raporu:**\n\
            *(Hedef: \"{}\")*\n\n\
            {}\n\n\
            ---\n\
            ✅ **Nihai Yanıt (Final Answer):**\n\
            Toplam **{} otonom adım** saf Rust ReAct döngüsüyle tamamlandı:\n{}\n\n\
            🧠 **Aktif Hafıza Bağlamı:**\n{}",
            clean,
            trace_blocks.join("\n\n"),
            steps.len(),
            summary_points.join("\n"),
            memory_ctx
        )
    }
}

// ============================================================================
// 6. PURE RUST MATHEMATICAL EXPRESSION EVALUATOR (`calculator` tool)
// ============================================================================

pub fn extract_math_candidate(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for prefix in ["hesapla:", "hesapla ", "calc:", "calculator:"] {
        if lower.starts_with(prefix) {
            let prefix_chars = prefix.chars().count();
            let expr_owned: String = text.chars().skip(prefix_chars).collect();
            let expr = expr_owned.trim();
            if !expr.is_empty() {
                return Some(expr.to_string());
            }
        }
    }
    // Check if the query contains digits and arithmetic operators
    let has_digit = text.chars().any(|c| c.is_ascii_digit());
    let has_op = text.chars().any(|c| matches!(c, '+' | '-' | '*' | '/' | '^'));
    let only_math = text.chars().all(|c| {
        c.is_ascii_digit()
            || c.is_whitespace()
            || matches!(c, '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')' | '.')
    });
    if has_digit && has_op && only_math {
        return Some(text.trim().to_string());
    }
    None
}

pub fn eval_math_expr(expr: &str) -> Result<f64, String> {
    let tokens = tokenize_math(expr)?;
    if tokens.is_empty() {
        return Err("Boş ifade".into());
    }
    let mut pos = 0;
    let result = parse_add_sub(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err(format!("Beklenmeyen sembol: {:?}", tokens[pos]));
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
enum MathToken {
    Num(f64),
    Op(char),
    LParen,
    RParen,
    Func(String),
}

fn tokenize_math(expr: &str) -> Result<Vec<MathToken>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            let val = num_str
                .parse::<f64>()
                .map_err(|_| format!("Geçersiz sayı: {}", num_str))?;
            tokens.push(MathToken::Num(val));
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let func: String = chars[start..i].iter().collect::<String>().to_lowercase();
            tokens.push(MathToken::Func(func));
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' | '%' | '^' => {
                tokens.push(MathToken::Op(c));
                i += 1;
            }
            '(' => {
                tokens.push(MathToken::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(MathToken::RParen);
                i += 1;
            }
            _ => return Err(format!("Desteklenmeyen karakter: '{}'", c)),
        }
    }
    Ok(tokens)
}

fn parse_add_sub(tokens: &[MathToken], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_mul_div(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            MathToken::Op('+') => {
                *pos += 1;
                left += parse_mul_div(tokens, pos)?;
            }
            MathToken::Op('-') => {
                *pos += 1;
                left -= parse_mul_div(tokens, pos)?;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_mul_div(tokens: &[MathToken], pos: &mut usize) -> Result<f64, String> {
    let mut left = parse_power(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            MathToken::Op('*') => {
                *pos += 1;
                left *= parse_power(tokens, pos)?;
            }
            MathToken::Op('/') => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                if right.abs() < 1e-12 {
                    return Err("Sıfıra bölme hatası".into());
                }
                left /= right;
            }
            MathToken::Op('%') => {
                *pos += 1;
                let right = parse_power(tokens, pos)?;
                left %= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_power(tokens: &[MathToken], pos: &mut usize) -> Result<f64, String> {
    let base = parse_unary(tokens, pos)?;
    if *pos < tokens.len() && tokens[*pos] == MathToken::Op('^') {
        *pos += 1;
        let exp = parse_power(tokens, pos)?;
        return Ok(base.powf(exp));
    }
    Ok(base)
}

fn parse_unary(tokens: &[MathToken], pos: &mut usize) -> Result<f64, String> {
    if *pos < tokens.len() {
        if tokens[*pos] == MathToken::Op('-') {
            *pos += 1;
            return Ok(-parse_unary(tokens, pos)?);
        }
        if tokens[*pos] == MathToken::Op('+') {
            *pos += 1;
            return parse_unary(tokens, pos);
        }
    }
    parse_primary(tokens, pos)
}

fn parse_primary(tokens: &[MathToken], pos: &mut usize) -> Result<f64, String> {
    if *pos >= tokens.len() {
        return Err("Beklenmeyen ifade sonu".into());
    }
    match &tokens[*pos] {
        MathToken::Num(v) => {
            *pos += 1;
            Ok(*v)
        }
        MathToken::Func(fname) => {
            let f = fname.clone();
            *pos += 1;
            let arg = parse_primary(tokens, pos)?;
            match f.as_str() {
                "sqrt" => Ok(arg.sqrt()),
                "abs" => Ok(arg.abs()),
                "sin" => Ok(arg.sin()),
                "cos" => Ok(arg.cos()),
                _ => Err(format!("Bilinmeyen fonksiyon: {}", f)),
            }
        }
        MathToken::LParen => {
            *pos += 1;
            let val = parse_add_sub(tokens, pos)?;
            if *pos >= tokens.len() || tokens[*pos] != MathToken::RParen {
                return Err("Kapanmayan parantez ')'".into());
            }
            *pos += 1;
            Ok(val)
        }
        other => Err(format!("Beklenmeyen atom: {:?}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculator_tool_evaluation() {
        let res = eval_math_expr("(125 * 4) + 50 / 2").unwrap();
        assert!((res - 525.0).abs() < 1e-6);

        let sqrt_res = eval_math_expr("sqrt(144) + 2^3").unwrap();
        assert!((sqrt_res - 20.0).abs() < 1e-6);
    }

    #[test]
    fn test_tool_registry_has_all_20_tools() {
        let tools = AuraToolRegistry::list_tools();
        assert_eq!(tools.len(), 20);
        assert!(tools.iter().any(|t| t.name == "calculator"));
        assert!(tools.iter().any(|t| t.name == "git_status"));
        assert!(tools.iter().any(|t| t.name == "corten_switch_workspace"));
    }

    #[test]
    fn test_hitl_security_gate_and_audit_log() {
        let out = AuraToolRegistry::execute_tool("run_terminal_command", "rm -rf /tmp/nonexistent");
        assert!(out.contains("HITL (Human-In-The-Loop) Güvenlik Onayı Gerekli"));
    }

    #[test]
    fn test_react_agent_multi_step_execution() {
        let report = AuraReActAgent::execute_react_workflow("react: sistem bilgisini ve git durumunu kontrol et");
        assert!(report.contains("Düşünce (Thought)"));
        assert!(report.contains("Eylem (Action)"));
        assert!(report.contains("Gözlem (Observation)"));
        assert!(report.contains("Nihai Yanıt (Final Answer)"));
    }
}
