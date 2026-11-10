use super::f16::f16_to_f32;

pub fn dequantize_q8_0(data: &[u8], num_elements: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; num_elements];
    let blocks = num_elements / 32;
    let block_size_bytes = 34;

    for b in 0..blocks {
        let offset = b * block_size_bytes;
        if offset + block_size_bytes > data.len() {
            break;
        }

        let block = &data[offset..offset + block_size_bytes];
        let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));

        let out_offset = b * 32;
        for i in 0..32 {
            let q = block[2 + i] as i8;
            if out_offset + i < output.len() {
                output[out_offset + i] = (q as f32) * d;
            }
        }
    }

    output
}
