use super::f16::f16_to_f32;

pub fn dequantize_q5_0(data: &[u8], num_elements: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; num_elements];
    let blocks = num_elements / 32;
    let block_size_bytes = 22;

    for b in 0..blocks {
        let offset = b * block_size_bytes;
        if offset + block_size_bytes > data.len() {
            break;
        }

        let block = &data[offset..offset + block_size_bytes];
        let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
        let qh = u32::from_le_bytes([block[2], block[3], block[4], block[5]]);

        let out_offset = b * 32;
        for i in 0..16 {
            let q_byte = block[6 + i];
            let bit0 = ((qh >> (2 * i)) & 1) as u8;
            let bit1 = ((qh >> (2 * i + 1)) & 1) as u8;

            let x0 = (q_byte & 0x0F) | (bit0 << 4);
            let x1 = (q_byte >> 4) | (bit1 << 4);

            if out_offset + i < output.len() {
                output[out_offset + i] = ((x0 as i32) - 16) as f32 * d;
            }
            if out_offset + i + 16 < output.len() {
                output[out_offset + i + 16] = ((x1 as i32) - 16) as f32 * d;
            }
        }
    }

    output
}
