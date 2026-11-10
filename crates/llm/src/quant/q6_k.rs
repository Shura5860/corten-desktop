use super::f16::f16_to_f32;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline(always)]
pub fn vec_dot_q6_k(x: &[f32], block: &[u8]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            unsafe {
                return vec_dot_q6_k_avx2(x, block);
            }
        }
    }
    vec_dot_q6_k_scalar(x, block)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
pub unsafe fn vec_dot_q6_k_avx2(x: &[f32], block: &[u8]) -> f32 {
    let ql = &block[0..128];
    let qh = &block[128..192];
    let scales = &block[192..208];
    let d = f16_to_f32(u16::from_le_bytes([block[208], block[209]]));

    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();

    for half in 0..2 {
        let ql_offset = half * 64;
        let qh_offset = half * 32;
        let scale_offset = half * 8;
        let x_offset = half * 128;

        let ql_ptr = ql.as_ptr().add(ql_offset);
        let qh_ptr = qh.as_ptr().add(qh_offset);
        let x_ptr = x.as_ptr().add(x_offset);

        for step in 0..4 {
            let l = step * 8;
            let is = (l) / 16;

            let sc0 = d * (scales[scale_offset + is + 0] as i8 as f32);
            let sc1 = d * (scales[scale_offset + is + 2] as i8 as f32);
            let sc2 = d * (scales[scale_offset + is + 4] as i8 as f32);
            let sc3 = d * (scales[scale_offset + is + 6] as i8 as f32);

            let v_sc0 = _mm256_set1_ps(sc0);
            let v_sc1 = _mm256_set1_ps(sc1);
            let v_sc2 = _mm256_set1_ps(sc2);
            let v_sc3 = _mm256_set1_ps(sc3);

            let mut w0_buf = [0.0f32; 8];
            let mut w1_buf = [0.0f32; 8];
            let mut w2_buf = [0.0f32; 8];
            let mut w3_buf = [0.0f32; 8];

            for k in 0..8 {
                let idx = l + k;
                let ql_l = *ql_ptr.add(idx);
                let ql_l32 = *ql_ptr.add(idx + 32);
                let qh_l = *qh_ptr.add(idx);

                let q1 = ((ql_l & 0xF) as i32) | ((((qh_l >> 0) & 3) as i32) << 4);
                let q2 = ((ql_l32 & 0xF) as i32) | ((((qh_l >> 2) & 3) as i32) << 4);
                let q3 = ((ql_l >> 4) as i32) | ((((qh_l >> 4) & 3) as i32) << 4);
                let q4 = ((ql_l32 >> 4) as i32) | ((((qh_l >> 6) & 3) as i32) << 4);

                w0_buf[k] = (q1 - 32) as f32;
                w1_buf[k] = (q2 - 32) as f32;
                w2_buf[k] = (q3 - 32) as f32;
                w3_buf[k] = (q4 - 32) as f32;
            }

            let v_w0 = _mm256_mul_ps(_mm256_loadu_ps(w0_buf.as_ptr()), v_sc0);
            let v_w1 = _mm256_mul_ps(_mm256_loadu_ps(w1_buf.as_ptr()), v_sc1);
            let v_w2 = _mm256_mul_ps(_mm256_loadu_ps(w2_buf.as_ptr()), v_sc2);
            let v_w3 = _mm256_mul_ps(_mm256_loadu_ps(w3_buf.as_ptr()), v_sc3);

            let v_x0 = _mm256_loadu_ps(x_ptr.add(l));
            let v_x1 = _mm256_loadu_ps(x_ptr.add(l + 32));
            let v_x2 = _mm256_loadu_ps(x_ptr.add(l + 64));
            let v_x3 = _mm256_loadu_ps(x_ptr.add(l + 96));

            acc0 = _mm256_fmadd_ps(v_x0, v_w0, acc0);
            acc1 = _mm256_fmadd_ps(v_x1, v_w1, acc1);
            acc0 = _mm256_fmadd_ps(v_x2, v_w2, acc0);
            acc1 = _mm256_fmadd_ps(v_x3, v_w3, acc1);
        }
    }

    let acc = _mm256_add_ps(acc0, acc1);
    hsum_float_avx(acc)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn hsum_float_avx(v: __m256) -> f32 {
    let vlow = _mm256_castps256_ps128(v);
    let vhigh = _mm256_extractf128_ps(v, 1);
    let v128 = _mm_add_ps(vlow, vhigh);
    let v64 = _mm_add_ps(v128, _mm_movehl_ps(v128, v128));
    let v32 = _mm_add_ss(v64, _mm_shuffle_ps(v64, v64, 0x55));
    _mm_cvtss_f32(v32)
}

pub fn vec_dot_q6_k_scalar(x: &[f32], block: &[u8]) -> f32 {
    let ql = &block[0..128];
    let qh = &block[128..192];
    let scales = &block[192..208];
    let d = f16_to_f32(u16::from_le_bytes([block[208], block[209]]));

    let mut sum = 0.0f32;

    for half in 0..2 {
        let ql_offset = half * 64;
        let qh_offset = half * 32;
        let scale_offset = half * 8;
        let x_offset = half * 128;

        for l in 0..32 {
            let is = l / 16;
            let sc0 = d * (scales[scale_offset + is + 0] as i8 as f32);
            let sc1 = d * (scales[scale_offset + is + 2] as i8 as f32);
            let sc2 = d * (scales[scale_offset + is + 4] as i8 as f32);
            let sc3 = d * (scales[scale_offset + is + 6] as i8 as f32);

            let ql_l = ql[ql_offset + l];
            let ql_l32 = ql[ql_offset + l + 32];
            let qh_l = qh[qh_offset + l];

            let q1 = ((ql_l & 0xF) as i32) | ((((qh_l >> 0) & 3) as i32) << 4);
            let q2 = ((ql_l32 & 0xF) as i32) | ((((qh_l >> 2) & 3) as i32) << 4);
            let q3 = ((ql_l >> 4) as i32) | ((((qh_l >> 4) & 3) as i32) << 4);
            let q4 = ((ql_l32 >> 4) as i32) | ((((qh_l >> 6) & 3) as i32) << 4);

            sum += x[x_offset + l] * (sc0 * ((q1 - 32) as f32));
            sum += x[x_offset + l + 32] * (sc1 * ((q2 - 32) as f32));
            sum += x[x_offset + l + 64] * (sc2 * ((q3 - 32) as f32));
            sum += x[x_offset + l + 96] * (sc3 * ((q4 - 32) as f32));
        }
    }

    sum
}

pub fn dequantize_q6_k(data: &[u8], num_elements: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; num_elements];
    let blocks = num_elements / 256;
    let block_size_bytes = 210;

    for b in 0..blocks {
        let offset = b * block_size_bytes;
        if offset + block_size_bytes > data.len() {
            break;
        }

        let block = &data[offset..offset + block_size_bytes];
        let ql = &block[0..128];
        let qh = &block[128..192];
        let scales = &block[192..208];
        let d = f16_to_f32(u16::from_le_bytes([block[208], block[209]]));

        let out_offset = b * 256;

        for half in 0..2 {
            let ql_offset = half * 64;
            let qh_offset = half * 32;
            let scale_offset = half * 8;
            let y_offset = out_offset + half * 128;

            for l in 0..32 {
                let is = l / 16;
                let sc0 = scales[scale_offset + is + 0] as i8 as f32;
                let sc1 = scales[scale_offset + is + 2] as i8 as f32;
                let sc2 = scales[scale_offset + is + 4] as i8 as f32;
                let sc3 = scales[scale_offset + is + 6] as i8 as f32;

                let ql_l = ql[ql_offset + l];
                let ql_l32 = ql[ql_offset + l + 32];
                let qh_l = qh[qh_offset + l];

                let q1 = ((ql_l & 0xF) as i32) | ((((qh_l >> 0) & 3) as i32) << 4);
                let q2 = ((ql_l32 & 0xF) as i32) | ((((qh_l >> 2) & 3) as i32) << 4);
                let q3 = ((ql_l >> 4) as i32) | ((((qh_l >> 4) & 3) as i32) << 4);
                let q4 = ((ql_l32 >> 4) as i32) | ((((qh_l >> 6) & 3) as i32) << 4);

                let idx0 = y_offset + l;
                let idx1 = y_offset + l + 32;
                let idx2 = y_offset + l + 64;
                let idx3 = y_offset + l + 96;

                if idx0 < output.len() {
                    output[idx0] = d * sc0 * ((q1 - 32) as f32);
                }
                if idx1 < output.len() {
                    output[idx1] = d * sc1 * ((q2 - 32) as f32);
                }
                if idx2 < output.len() {
                    output[idx2] = d * sc2 * ((q3 - 32) as f32);
                }
                if idx3 < output.len() {
                    output[idx3] = d * sc3 * ((q4 - 32) as f32);
                }
            }
        }
    }

    output
}
