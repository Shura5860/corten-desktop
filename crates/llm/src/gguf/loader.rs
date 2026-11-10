use anyhow::{anyhow, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

use crate::engine::weights::QuantizedTensor;
use crate::quant::{dequantize_q4_k, dequantize_q5_0, dequantize_q6_k, dequantize_q8_0, f16_to_f32};
use super::header::GgufHeader;

pub fn load_quantized_tensor(
    header: &GgufHeader,
    mmap: &[u8],
    tensor_data_offset: usize,
    tensor_name: &str,
) -> Result<QuantizedTensor> {
    let tensor = header
        .tensors
        .iter()
        .find(|t| t.name == tensor_name)
        .ok_or_else(|| anyhow!("Tensor bulunamadı: {}", tensor_name))?;

    let num_elements: usize = tensor.dimensions.iter().map(|&d| d as usize).product();
    let start = tensor_data_offset + tensor.offset as usize;
    let bytes = &mmap[start..];

    if tensor.qtype == 0 {
        let mut cursor = Cursor::new(bytes);
        let mut res = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            res.push(cursor.read_f32::<LittleEndian>()?);
        }
        Ok(QuantizedTensor::F32(res))
    } else if tensor.qtype == 1 {
        let mut cursor = Cursor::new(bytes);
        let mut res = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            let h = cursor.read_u16::<LittleEndian>()?;
            res.push(f16_to_f32(h));
        }
        Ok(QuantizedTensor::F32(res))
    } else if tensor.qtype == 12 {
        let blocks = num_elements / 256;
        let bytes_len = blocks * 144;
        Ok(QuantizedTensor::Q4K { data: bytes[..bytes_len].to_vec() })
    } else if tensor.qtype == 14 {
        let blocks = num_elements / 256;
        let bytes_len = blocks * 210;
        Ok(QuantizedTensor::Q6K { data: bytes[..bytes_len].to_vec() })
    } else {
        let f32_vec = if tensor.qtype == 6 {
            dequantize_q5_0(bytes, num_elements)
        } else if tensor.qtype == 8 {
            dequantize_q8_0(bytes, num_elements)
        } else {
            dequantize_q4_k(bytes, num_elements)
        };
        Ok(QuantizedTensor::F32(f32_vec))
    }
}

pub fn load_tensor_data(
    header: &GgufHeader,
    mmap: &[u8],
    tensor_data_offset: usize,
    tensor_name: &str,
) -> Result<Vec<f32>> {
    let qt = load_quantized_tensor(header, mmap, tensor_data_offset, tensor_name)?;
    match qt {
        QuantizedTensor::F32(v) => Ok(v),
        QuantizedTensor::Q4K { data } => Ok(dequantize_q4_k(&data, data.len() / 144 * 256)),
        QuantizedTensor::Q6K { data } => Ok(dequantize_q6_k(&data, data.len() / 210 * 256)),
    }
}

pub fn get_token_embedding(embd: &QuantizedTensor, token: usize, hidden_dim: usize, _vocab_size: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; hidden_dim];
    match embd {
        QuantizedTensor::F32(v) => {
            let offset = token * hidden_dim;
            if offset + hidden_dim <= v.len() {
                vec.copy_from_slice(&v[offset..offset + hidden_dim]);
            }
        }
        QuantizedTensor::Q6K { data } => {
            let blocks_per_row = hidden_dim / 256;
            let row_bytes = blocks_per_row * 210;
            let offset = token * row_bytes;
            if offset + row_bytes <= data.len() {
                let row_data = &data[offset..offset + row_bytes];
                let dequant = dequantize_q6_k(row_data, hidden_dim);
                vec.copy_from_slice(&dequant);
            }
        }
        QuantizedTensor::Q4K { data } => {
            let blocks_per_row = hidden_dim / 256;
            let row_bytes = blocks_per_row * 144;
            let offset = token * row_bytes;
            if offset + row_bytes <= data.len() {
                let row_data = &data[offset..offset + row_bytes];
                let dequant = dequantize_q4_k(row_data, hidden_dim);
                vec.copy_from_slice(&dequant);
            }
        }
    }
    vec
}
