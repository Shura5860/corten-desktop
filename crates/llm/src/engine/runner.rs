use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::gguf::{get_token_embedding, load_quantized_tensor, load_tensor_data, GgufHeader, GgufValue};
use crate::ops::{apply_repetition_penalty, rms_norm_into, sample_token, SamplerParams};
use crate::tokenizer::SimpleTokenizer;
use super::block::{ScratchBuffer, TransformerBlock};
use super::weights::{KVCache, LayerWeights, QuantizedTensor};

pub enum InferenceMessage {
    Token(String),
    Metrics {
        prefill_tok_s: f64,
        decode_tok_s: f64,
        ms_per_tok: f64,
        total_gen_tokens: usize,
    },
    Finished,
    Error(String),
}

pub struct ModelRunner {
    pub header: GgufHeader,
    pub tokenizer: SimpleTokenizer,
    pub hidden_dim: usize,
    pub vocab_size: usize,
    pub ffn_dim: usize,
    pub num_layers: usize,
    pub num_heads: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
    pub rope_theta: f32,
    pub eos_token_id: usize,
    pub stop_token_ids: HashSet<usize>,
    pub stop_strings: Vec<&'static str>,
    pub embd_weights: QuantizedTensor,
    pub output_weights: QuantizedTensor,
    pub output_norm_weight: Vec<f32>,
    pub transformer_layers: Vec<TransformerBlock>,
}

impl ModelRunner {
    pub fn load_from_file(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };
        let mut cursor = std::io::Cursor::new(&mmap[..]);
        let header = GgufHeader::parse(&mut cursor)?;
        let alignment = header
            .metadata
            .get("general.alignment")
            .and_then(|v| v.to_u64())
            .unwrap_or(32) as usize;
        let pos = cursor.position() as usize;
        let tensor_data_offset = (pos + alignment - 1) & !(alignment - 1);
        Self::load(header, &mmap, tensor_data_offset)
    }

    pub fn load(header: GgufHeader, mmap: &[u8], tensor_data_offset: usize) -> Result<Self> {
        let num_layers = header
            .metadata
            .get("llama.block_count")
            .or_else(|| header.metadata.get("qwen2.block_count"))
            .and_then(|v| v.to_u64())
            .unwrap_or(30) as usize;

        let tokenizer = if let Some(GgufValue::Array(tokens)) = header.metadata.get("tokenizer.ggml.tokens") {
            SimpleTokenizer::new(tokens)
        } else {
            return Err(anyhow!("Tokenizer tokens bulunamadı."));
        };

        let eos_token_id = header
            .metadata
            .get("tokenizer.ggml.eos_token_id")
            .and_then(|v| v.to_u64())
            .map(|v| v as usize)
            .unwrap_or(128009);

        let mut stop_token_ids: HashSet<usize> = HashSet::new();
        stop_token_ids.insert(eos_token_id);
        stop_token_ids.insert(0);      // UNK
        stop_token_ids.insert(151643); // Qwen2 <|endoftext|>
        stop_token_ids.insert(151645); // Qwen2 <|im_end|>
        stop_token_ids.insert(128001); // Llama 3 <|end_of_text|>
        stop_token_ids.insert(128009); // Llama 3 <|eot_id|>

        let stop_strings = vec!["<|endoftext|>", "<|im_end|>", "<|end_of_text|>", "<|eot_id|>", "<|end|>"];

        let embd_tensor = header
            .tensors
            .iter()
            .find(|t| t.name == "token_embd.weight")
            .ok_or_else(|| anyhow!("token_embd.weight bulunamadı"))?;

        let dim0 = embd_tensor.dimensions[0] as usize;
        let dim1 = embd_tensor.dimensions[1] as usize;

        let (hidden_dim, vocab_size) = if dim0 < dim1 {
            (dim0, dim1)
        } else {
            (dim1, dim0)
        };

        let ffn_dim = header
            .metadata
            .get("llama.feed_forward_length")
            .or_else(|| header.metadata.get("qwen2.feed_forward_length"))
            .and_then(|v| v.to_u64())
            .map(|v| v as usize)
            .unwrap_or(11008);

        let num_heads = header
            .metadata
            .get("llama.attention.head_count")
            .or_else(|| header.metadata.get("qwen2.attention.head_count"))
            .and_then(|v| v.to_u64())
            .unwrap_or(32) as usize;

        let num_kv_heads = header
            .metadata
            .get("llama.attention.head_count_kv")
            .or_else(|| header.metadata.get("qwen2.attention.head_count_kv"))
            .and_then(|v| v.to_u64())
            .map(|v| v as usize)
            .unwrap_or(num_heads);

        let rope_theta = header
            .metadata
            .get("llama.rope.freq_base")
            .or_else(|| header.metadata.get("qwen2.rope.freq_base"))
            .and_then(|v| v.to_u64())
            .map(|v| v as f32)
            .unwrap_or(500000.0f32);

        let head_dim = hidden_dim / num_heads;

        let embd_weights = load_quantized_tensor(&header, mmap, tensor_data_offset, "token_embd.weight")?;

        let output_weights = if let Some(t) = header.tensors.iter().find(|t| t.name == "output.weight") {
            println!("📐 output.weight found: dims={:?}, qtype={}", t.dimensions, t.qtype);
            load_quantized_tensor(&header, mmap, tensor_data_offset, "output.weight")?
        } else {
            println!("📐 output.weight tied to token_embd.weight");
            embd_weights.clone()
        };

        println!("📐 token_embd.weight qtype={}", embd_tensor.qtype);

        let output_norm_weight = load_tensor_data(&header, mmap, tensor_data_offset, "output_norm.weight")?;

        println!("🧠 Model Config: num_layers={}, hidden_dim={}, ffn_dim={}, vocab_size={}, num_heads={}, num_kv_heads={}, head_dim={}, rope_theta={}", num_layers, hidden_dim, ffn_dim, vocab_size, num_heads, num_kv_heads, head_dim, rope_theta);
        println!("🧠 {} Katman Yükleniyor... (Hidden: {}, Vocab: {}, Heads: Q:{}/KV:{}, RoPE: {})", num_layers, hidden_dim, vocab_size, num_heads, num_kv_heads, rope_theta);

        let mut transformer_layers = Vec::with_capacity(num_layers);
        for layer_idx in 0..num_layers {
            print!("\r   [Yükleme]: {}/{}", layer_idx + 1, num_layers);
            io::stdout().flush()?;

            let prefix = format!("blk.{}.", layer_idx);
            let attn_norm = load_tensor_data(&header, mmap, tensor_data_offset, &format!("{}attn_norm.weight", prefix))?;
            let wq = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}attn_q.weight", prefix))?;
            let wk = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}attn_k.weight", prefix))?;
            let wv = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}attn_v.weight", prefix))?;
            let wo = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}attn_output.weight", prefix))?;

            let ffn_norm = load_tensor_data(&header, mmap, tensor_data_offset, &format!("{}ffn_norm.weight", prefix))?;
            let w_gate = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}ffn_gate.weight", prefix))?;
            let w_up = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}ffn_up.weight", prefix))?;
            let w_down = load_quantized_tensor(&header, mmap, tensor_data_offset, &format!("{}ffn_down.weight", prefix))?;

            let block = TransformerBlock {
                hidden_dim,
                ffn_dim,
                num_heads,
                num_kv_heads,
                head_dim,
                rope_theta,
                weights: LayerWeights {
                    attn_norm,
                    wq,
                    wk,
                    wv,
                    wo,
                    ffn_norm,
                    w_gate,
                    w_up,
                    w_down,
                },
            };
            transformer_layers.push(block);
        }
        println!("\n⚡ Katmanlar Hazır! Üretim Başlıyor:\n");

        Ok(Self {
            header,
            tokenizer,
            hidden_dim,
            vocab_size,
            ffn_dim,
            num_layers,
            num_heads,
            num_kv_heads,
            head_dim,
            rope_theta,
            eos_token_id,
            stop_token_ids,
            stop_strings,
            embd_weights,
            output_weights,
            output_norm_weight,
            transformer_layers,
        })
    }

    pub fn generate(&self, prompt_text: &str, max_tokens: usize, temperature: f32, top_p: f32) -> Result<()> {
        let params = SamplerParams {
            temperature,
            top_k: 40,
            top_p,
            repetition_penalty: 1.1,
        };
        self.generate_with_params(prompt_text, max_tokens, &params)
    }

    pub fn generate_string(&self, prompt_text: &str, max_tokens: usize, params: &SamplerParams) -> Result<String> {
        let mut kv_caches: Vec<KVCache> = (0..self.num_layers).map(|_| KVCache::new()).collect();
        let prompt_tokens = self.tokenizer.encode(prompt_text);

        let mut scratch = ScratchBuffer::new(
            self.hidden_dim,
            self.ffn_dim,
            self.num_heads,
            self.num_kv_heads,
            self.head_dim,
            self.vocab_size,
        );

        let mut last_x = vec![0.0f32; self.hidden_dim];
        for (pos, &token) in prompt_tokens.iter().enumerate() {
            let mut x = get_token_embedding(&self.embd_weights, token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }
            last_x = x;
        }

        rms_norm_into(&mut scratch.x_norm, &last_x, &self.output_norm_weight, 1e-5);
        self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

        let mut current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);
        let start_pos = prompt_tokens.len();
        let mut generated_tokens = prompt_tokens.clone();
        let mut output_str = String::new();

        for pos in start_pos..(start_pos + max_tokens) {
            if self.stop_token_ids.contains(&current_token) {
                break;
            }

            let token_text = self.tokenizer.decode(current_token as u32);
            if self.stop_strings.iter().any(|&s| token_text.contains(s)) {
                break;
            }

            output_str.push_str(&token_text);
            generated_tokens.push(current_token);

            let mut x = get_token_embedding(&self.embd_weights, current_token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }

            rms_norm_into(&mut scratch.x_norm, &x, &self.output_norm_weight, 1e-5);
            self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

            apply_repetition_penalty(&mut scratch.logits, &generated_tokens, params.repetition_penalty);
            current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);
        }

        Ok(output_str)
    }

    pub fn generate_with_params(&self, prompt_text: &str, max_tokens: usize, params: &SamplerParams) -> Result<()> {
        let mut kv_caches: Vec<KVCache> = (0..self.num_layers).map(|_| KVCache::new()).collect();
        let prompt_tokens = self.tokenizer.encode(prompt_text);

        let mut scratch = ScratchBuffer::new(
            self.hidden_dim,
            self.ffn_dim,
            self.num_heads,
            self.num_kv_heads,
            self.head_dim,
            self.vocab_size,
        );

        // 1. PREFILL STAGE
        let prefill_start = Instant::now();
        let mut last_x = vec![0.0f32; self.hidden_dim];

        for (pos, &token) in prompt_tokens.iter().enumerate() {
            let mut x = get_token_embedding(&self.embd_weights, token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }
            last_x = x;
        }

        rms_norm_into(&mut scratch.x_norm, &last_x, &self.output_norm_weight, 1e-5);
        self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

        let prefill_elapsed = prefill_start.elapsed();
        let prefill_tok_s = prompt_tokens.len() as f64 / prefill_elapsed.as_secs_f64();

        let mut current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);

        // CANLI AKIŞ (STREAMING)
        print!("{}", prompt_text);
        io::stdout().flush()?;

        let start_pos = prompt_tokens.len();
        let mut generated_tokens = prompt_tokens.clone();

        // 2. DECODE STAGE
        let decode_start = Instant::now();
        let mut generated_count = 0;

        for pos in start_pos..(start_pos + max_tokens) {
            if self.stop_token_ids.contains(&current_token) {
                break;
            }

            let token_text = self.tokenizer.decode(current_token as u32);
            if self.stop_strings.iter().any(|&s| token_text.contains(s)) {
                break;
            }

            print!("{}", token_text);
            io::stdout().flush()?;

            generated_tokens.push(current_token);
            generated_count += 1;

            let mut x = get_token_embedding(&self.embd_weights, current_token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }

            rms_norm_into(&mut scratch.x_norm, &x, &self.output_norm_weight, 1e-5);
            self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

            apply_repetition_penalty(&mut scratch.logits, &generated_tokens, params.repetition_penalty);
            current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);
        }

        let decode_elapsed = decode_start.elapsed();
        let decode_tok_s = if generated_count > 0 {
            generated_count as f64 / decode_elapsed.as_secs_f64()
        } else {
            0.0
        };
        let ms_per_tok = if generated_count > 0 {
            decode_elapsed.as_secs_f64() * 1000.0 / generated_count as f64
        } else {
            0.0
        };

        println!("\n\n✅ Üretim Tamamlandı!");
        println!("--------------------------------------------------");
        println!("📊 PERFORMANS METRİKLERİ (FP16 KV-Cache & Sampler Pipeline):");
        println!("  • Prefill Hızı:  {} token | {:.2} ms ({:.2} tok/s)", prompt_tokens.len(), prefill_elapsed.as_secs_f64() * 1000.0, prefill_tok_s);
        println!("  • Decode Hızı:   {} token | {:.2} ms ({:.2} tok/s | {:.2} ms/token)", generated_count, decode_elapsed.as_secs_f64() * 1000.0, decode_tok_s, ms_per_tok);
        println!("--------------------------------------------------");

        Ok(())
    }

    pub fn generate_tui_stream(
        &self,
        prompt_text: &str,
        max_tokens: usize,
        params: &SamplerParams,
        tx: &Sender<InferenceMessage>,
    ) -> Result<()> {
        let mut kv_caches: Vec<KVCache> = (0..self.num_layers).map(|_| KVCache::new()).collect();
        let prompt_tokens = self.tokenizer.encode(prompt_text);

        let mut scratch = ScratchBuffer::new(
            self.hidden_dim,
            self.ffn_dim,
            self.num_heads,
            self.num_kv_heads,
            self.head_dim,
            self.vocab_size,
        );

        let prefill_start = Instant::now();
        let mut last_x = vec![0.0f32; self.hidden_dim];

        for (pos, &token) in prompt_tokens.iter().enumerate() {
            let mut x = get_token_embedding(&self.embd_weights, token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }
            last_x = x;
        }

        rms_norm_into(&mut scratch.x_norm, &last_x, &self.output_norm_weight, 1e-5);
        self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

        let prefill_elapsed = prefill_start.elapsed();
        let prefill_tok_s = prompt_tokens.len() as f64 / prefill_elapsed.as_secs_f64();

        let mut current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);

        let start_pos = prompt_tokens.len();
        let mut generated_tokens = prompt_tokens.clone();

        let decode_start = Instant::now();
        let mut generated_count = 0;

        for pos in start_pos..(start_pos + max_tokens) {
            if self.stop_token_ids.contains(&current_token) {
                break;
            }

            let token_text = self.tokenizer.decode(current_token as u32);
            if self.stop_strings.iter().any(|&s| token_text.contains(s)) {
                break;
            }

            let _ = tx.send(InferenceMessage::Token(token_text));

            generated_tokens.push(current_token);
            generated_count += 1;

            let mut x = get_token_embedding(&self.embd_weights, current_token, self.hidden_dim, self.vocab_size);
            for (i, block) in self.transformer_layers.iter().enumerate() {
                block.forward(&mut x, pos, &mut kv_caches[i], &mut scratch);
            }

            rms_norm_into(&mut scratch.x_norm, &x, &self.output_norm_weight, 1e-5);
            self.output_weights.mat_vec_mul_into(&mut scratch.logits, &scratch.x_norm, self.vocab_size, self.hidden_dim);

            apply_repetition_penalty(&mut scratch.logits, &generated_tokens, params.repetition_penalty);
            current_token = sample_token(&mut scratch.logits, params, &self.stop_token_ids);

            let decode_elapsed = decode_start.elapsed();
            let decode_tok_s = generated_count as f64 / decode_elapsed.as_secs_f64();
            let ms_per_tok = decode_elapsed.as_secs_f64() * 1000.0 / generated_count as f64;

            let _ = tx.send(InferenceMessage::Metrics {
                prefill_tok_s,
                decode_tok_s,
                ms_per_tok,
                total_gen_tokens: generated_count,
            });
        }

        let _ = tx.send(InferenceMessage::Finished);
        Ok(())
    }
}
