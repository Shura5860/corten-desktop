pub fn apply_rope(vec: &mut [f32], pos: usize, num_heads: usize, head_dim: usize, theta_base: f32) {
    let half = head_dim / 2;
    for h in 0..num_heads {
        let head_offset = h * head_dim;
        for i in 0..half {
            let freq = 1.0 / (theta_base as f64).powf((2 * i) as f64 / head_dim as f64) as f32;
            let val = (pos as f32) * freq;
            let cos_val = val.cos();
            let sin_val = val.sin();

            let idx0 = head_offset + i * 2;
            let idx1 = head_offset + i * 2 + 1;

            if idx1 < vec.len() {
                let v0 = vec[idx0];
                let v1 = vec[idx1];

                vec[idx0] = v0 * cos_val - v1 * sin_val;
                vec[idx1] = v0 * sin_val + v1 * cos_val;
            }
        }
    }
}
