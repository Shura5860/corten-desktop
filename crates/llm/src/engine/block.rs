use crate::ops::{apply_rope, rms_norm_into, silu, softmax};
use crate::quant::{f16_to_f32, f32_to_f16};
use super::weights::{KVCache, LayerWeights};

pub struct ScratchBuffer {
    pub x_norm: Vec<f32>,
    pub q: Vec<f32>,
    pub k: Vec<f32>,
    pub v: Vec<f32>,
    pub attn_out: Vec<f32>,
    pub scores: Vec<f32>,
    pub wo_out: Vec<f32>,
    pub gate: Vec<f32>,
    pub up: Vec<f32>,
    pub hidden_ffn: Vec<f32>,
    pub ffn_out: Vec<f32>,
    pub logits: Vec<f32>,
}

impl ScratchBuffer {
    pub fn new(hidden_dim: usize, ffn_dim: usize, num_heads: usize, num_kv_heads: usize, head_dim: usize, vocab_size: usize) -> Self {
        let kv_dim = num_kv_heads * head_dim;
        let q_dim = num_heads * head_dim;
        Self {
            x_norm: vec![0.0f32; hidden_dim],
            q: vec![0.0f32; q_dim],
            k: vec![0.0f32; kv_dim],
            v: vec![0.0f32; kv_dim],
            attn_out: vec![0.0f32; q_dim],
            scores: vec![0.0f32; 2048],
            wo_out: vec![0.0f32; hidden_dim],
            gate: vec![0.0f32; ffn_dim],
            up: vec![0.0f32; ffn_dim],
            hidden_ffn: vec![0.0f32; ffn_dim],
            ffn_out: vec![0.0f32; hidden_dim],
            logits: vec![0.0f32; vocab_size],
        }
    }
}

pub struct TransformerBlock {
    pub weights: LayerWeights,
    pub hidden_dim: usize,
    pub ffn_dim: usize,
    pub num_heads: usize,
    pub num_kv_heads: usize,
    pub head_dim: usize,
    pub rope_theta: f32,
}

impl TransformerBlock {
    pub fn forward(&self, x: &mut [f32], pos: usize, kv_cache: &mut KVCache, scratch: &mut ScratchBuffer) {
        let eps = 1e-5;
        let kv_dim = self.num_kv_heads * self.head_dim;

        // 1. ATTENTION
        rms_norm_into(&mut scratch.x_norm, x, &self.weights.attn_norm, eps);

        self.weights.wq.mat_vec_mul_into(&mut scratch.q, &scratch.x_norm, self.num_heads * self.head_dim, self.hidden_dim);
        self.weights.wk.mat_vec_mul_into(&mut scratch.k, &scratch.x_norm, kv_dim, self.hidden_dim);
        self.weights.wv.mat_vec_mul_into(&mut scratch.v, &scratch.x_norm, kv_dim, self.hidden_dim);

        apply_rope(&mut scratch.q, pos, self.num_heads, self.head_dim, self.rope_theta);
        apply_rope(&mut scratch.k, pos, self.num_kv_heads, self.head_dim, self.rope_theta);

        // Compress K & V to FP16 (u16)
        let k_fp16: Vec<u16> = scratch.k.iter().map(|&val| f32_to_f16(val)).collect();
        let v_fp16: Vec<u16> = scratch.v.iter().map(|&val| f32_to_f16(val)).collect();

        kv_cache.keys.push(k_fp16);
        kv_cache.values.push(v_fp16);

        let seq_len = kv_cache.keys.len();
        let scale = 1.0 / (self.head_dim as f32).sqrt();
        let n_rep = self.num_heads / self.num_kv_heads.max(1);

        scratch.attn_out.fill(0.0);

        if scratch.scores.len() < seq_len {
            scratch.scores.resize(seq_len, 0.0);
        }

        for h in 0..self.num_heads {
            let h_kv = h / n_rep;
            let q_offset = h * self.head_dim;
            let kv_offset = h_kv * self.head_dim;
            let q_head = &scratch.q[q_offset..q_offset + self.head_dim];

            for t in 0..seq_len {
                let k_head_fp16 = &kv_cache.keys[t][kv_offset..kv_offset + self.head_dim];
                let mut score = 0.0f32;
                for d in 0..self.head_dim {
                    score += q_head[d] * f16_to_f32(k_head_fp16[d]);
                }
                scratch.scores[t] = score * scale;
            }

            softmax(&mut scratch.scores[..seq_len]);

            for t in 0..seq_len {
                let v_head_fp16 = &kv_cache.values[t][kv_offset..kv_offset + self.head_dim];
                let score = scratch.scores[t];
                for d in 0..self.head_dim {
                    scratch.attn_out[q_offset + d] += score * f16_to_f32(v_head_fp16[d]);
                }
            }
        }

        self.weights.wo.mat_vec_mul_into(&mut scratch.wo_out, &scratch.attn_out, self.hidden_dim, self.num_heads * self.head_dim);

        for i in 0..self.hidden_dim {
            x[i] += scratch.wo_out[i];
        }

        // 2. FFN
        rms_norm_into(&mut scratch.x_norm, x, &self.weights.ffn_norm, eps);

        self.weights.w_gate.mat_vec_mul_into(&mut scratch.gate, &scratch.x_norm, self.ffn_dim, self.hidden_dim);
        self.weights.w_up.mat_vec_mul_into(&mut scratch.up, &scratch.x_norm, self.ffn_dim, self.hidden_dim);

        for i in 0..self.ffn_dim {
            scratch.hidden_ffn[i] = silu(scratch.gate[i]) * scratch.up[i];
        }

        self.weights.w_down.mat_vec_mul_into(&mut scratch.ffn_out, &scratch.hidden_ffn, self.hidden_dim, self.ffn_dim);

        for i in 0..self.hidden_dim {
            x[i] += scratch.ffn_out[i];
        }
    }
}
