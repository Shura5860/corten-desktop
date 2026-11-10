use rayon::prelude::*;
use crate::ops::dot_product_simd;
use crate::quant::{vec_dot_q4_k, vec_dot_q6_k};

#[derive(Clone)]
pub enum QuantizedTensor {
    F32(Vec<f32>),
    Q4K { data: Vec<u8> },
    Q6K { data: Vec<u8> },
}

impl QuantizedTensor {
    pub fn mat_vec_mul_into(&self, out: &mut [f32], x: &[f32], rows: usize, cols: usize) {
        match self {
            QuantizedTensor::F32(matrix) => {
                out[..rows].par_iter_mut().enumerate().for_each(|(r, out_val)| {
                    let row_offset = r * cols;
                    if row_offset + cols <= matrix.len() {
                        let row_slice = &matrix[row_offset..row_offset + cols];
                        *out_val = dot_product_simd(row_slice, x);
                    }
                });
            }
            QuantizedTensor::Q4K { data } => {
                let blocks_per_row = cols / 256;
                let row_bytes = blocks_per_row * 144;

                out[..rows].par_iter_mut().enumerate().for_each(|(r, out_val)| {
                    let offset = r * row_bytes;
                    if offset + row_bytes <= data.len() {
                        let row_data = &data[offset..offset + row_bytes];
                        let mut sum = 0.0f32;

                        for b in 0..blocks_per_row {
                            let block = &row_data[b * 144..(b + 1) * 144];
                            let x_slice = &x[b * 256..(b + 1) * 256];
                            sum += vec_dot_q4_k(x_slice, block);
                        }
                        *out_val = sum;
                    }
                });
            }
            QuantizedTensor::Q6K { data } => {
                let blocks_per_row = cols / 256;
                let row_bytes = blocks_per_row * 210;

                out[..rows].par_iter_mut().enumerate().for_each(|(r, out_val)| {
                    let offset = r * row_bytes;
                    if offset + row_bytes <= data.len() {
                        let row_data = &data[offset..offset + row_bytes];
                        let mut sum = 0.0f32;

                        for b in 0..blocks_per_row {
                            let block = &row_data[b * 210..(b + 1) * 210];
                            let x_slice = &x[b * 256..(b + 1) * 256];
                            sum += vec_dot_q6_k(x_slice, block);
                        }
                        *out_val = sum;
                    }
                });
            }
        }
    }
}

pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedTensor,
    pub wk: QuantizedTensor,
    pub wv: QuantizedTensor,
    pub wo: QuantizedTensor,
    pub ffn_norm: Vec<f32>,
    pub w_gate: QuantizedTensor,
    pub w_up: QuantizedTensor,
    pub w_down: QuantizedTensor,
}

pub struct KVCache {
    pub keys: Vec<Vec<u16>>,
    pub values: Vec<Vec<u16>>,
}

impl KVCache {
    pub fn new() -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
        }
    }
}
