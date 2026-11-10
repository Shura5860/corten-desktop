#[inline]
pub fn rms_norm_into(out: &mut [f32], x: &[f32], weight: &[f32], eps: f32) {
    let sum_sq: f32 = x.iter().map(|&v| v * v).sum();
    let mean_sq = sum_sq / (x.len() as f32);
    let scale = 1.0 / (mean_sq + eps).sqrt();
    for i in 0..x.len() {
        out[i] = x[i] * scale * weight[i];
    }
}

pub fn rms_norm(x: &[f32], weight: &[f32], eps: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; x.len()];
    rms_norm_into(&mut out, x, weight, eps);
    out
}
