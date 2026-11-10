#![allow(clippy::all, dead_code)]

pub mod engine;
pub mod gguf;
pub mod ops;
pub mod quant;
pub mod tokenizer;

pub use engine::{InferenceMessage, KVCache, LayerWeights, ModelRunner, TransformerBlock};
pub use gguf::{GgufHeader, GgufValue, TensorInfo, GGUF_MAGIC};
pub use ops::{rms_norm_into, apply_rope, sample_token, SamplerParams};
pub use tokenizer::SimpleTokenizer;

/// Scans standard XDG and Corten model directories for local `.gguf` files
pub fn discover_local_gguf_models() -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut dirs = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        let h = std::path::PathBuf::from(home);
        dirs.push(h.join(".local/share/corten/models"));
        dirs.push(h.join("Models"));
        dirs.push(h.join("Projects/AIProject/Rust-LLM-Engine/models"));
        dirs.push(h.join("Projects/AIProject/release_bundle/models"));
    }
    dirs.push(std::path::PathBuf::from("/usr/share/corten/models"));
    dirs.push(std::path::PathBuf::from("/usr/local/share/corten/models"));

    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        if ext.eq_ignore_ascii_case("gguf") {
                            found.push(p);
                        }
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_gguf_magic_validation() {
        let invalid_bytes = [0x00, 0x01, 0x02, 0x03, 0x03, 0x00, 0x00, 0x00];
        let mut cursor = Cursor::new(&invalid_bytes[..]);
        assert!(GgufHeader::parse(&mut cursor).is_err());
    }

    #[test]
    fn test_rms_norm_computation() {
        let x = vec![1.0f32, 2.0, 3.0, 4.0];
        let weight = vec![1.0f32, 1.0, 1.0, 1.0];
        let mut out = vec![0.0f32; 4];
        rms_norm_into(&mut out, &x, &weight, 1e-5);
        // Mean of squares = (1+4+9+16)/4 = 7.5, rms = sqrt(7.5) ≈ 2.7386
        assert!((out[0] - (1.0 / 7.5f32.sqrt())).abs() < 1e-4);
    }

    #[test]
    fn test_simple_tokenizer_roundtrip() {
        let vocab = vec![
            GgufValue::String("<unk>".into()),
            GgufValue::String("merhaba".into()),
            GgufValue::String(" ".into()),
            GgufValue::String("corten".into()),
        ];
        let tokenizer = SimpleTokenizer::new(&vocab);
        let ids = tokenizer.encode("merhaba corten");
        assert!(!ids.is_empty());
        let decoded: String = ids.iter().map(|&id| tokenizer.decode(id as u32)).collect();
        assert_eq!(decoded, "merhaba corten");
    }
}

