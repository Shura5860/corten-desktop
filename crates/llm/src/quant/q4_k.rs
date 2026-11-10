use super::f16::f16_to_f32;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline(always)]
pub fn vec_dot_q4_k(x: &[f32], block: &[u8]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            unsafe {
                return vec_dot_q4_k_avx2(x, block);
            }
        }
    }
    vec_dot_q4_k_scalar(x, block)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
pub unsafe fn vec_dot_q4_k_avx2(x: &[f32], block: &[u8]) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let dmin = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));

    let scales = &block[4..16];
    let qs = &block[16..144];

    let get_scale_min = |is: usize| -> (f32, f32) {
        let sc: u8;
        let m: u8;
        if is < 4 {
            sc = scales[is] & 63;
            m = scales[is + 4] & 63;
        } else {
            sc = (scales[is + 4] & 0xF) | ((scales[is - 4] >> 6) << 4);
            m = (scales[is + 4] >> 4) | ((scales[is] >> 6) << 4);
        }
        (d * (sc as f32), dmin * (m as f32))
    };

    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();

    for j in 0..4 {
        let (sc0, m0) = get_scale_min(2 * j);
        let (sc1, m1) = get_scale_min(2 * j + 1);

        let v_sc0 = _mm256_set1_ps(sc0);
        let v_m0 = _mm256_set1_ps(m0);
        let v_sc1 = _mm256_set1_ps(sc1);
        let v_m1 = _mm256_set1_ps(m1);

        let q_offset = j * 32;
        let x_offset0 = j * 64;
        let x_offset1 = x_offset0 + 32;

        let qs_ptr = qs.as_ptr().add(q_offset);
        let x0_ptr = x.as_ptr().add(x_offset0);
        let x1_ptr = x.as_ptr().add(x_offset1);

        for step in 0..4 {
            let l = step * 8;

            let mut w0_buf = [0.0f32; 8];
            let mut w1_buf = [0.0f32; 8];

            for k in 0..8 {
                let q_byte = *qs_ptr.add(l + k);
                w0_buf[k] = (q_byte & 0x0F) as f32;
                w1_buf[k] = (q_byte >> 4) as f32;
            }

            let v_q0 = _mm256_loadu_ps(w0_buf.as_ptr());
            let v_q1 = _mm256_loadu_ps(w1_buf.as_ptr());

            let v_w0 = _mm256_fmsub_ps(v_q0, v_sc0, v_m0);
            let v_w1 = _mm256_fmsub_ps(v_q1, v_sc1, v_m1);

            let v_x0 = _mm256_loadu_ps(x0_ptr.add(l));
            let v_x1 = _mm256_loadu_ps(x1_ptr.add(l));

            acc0 = _mm256_fmadd_ps(v_x0, v_w0, acc0);
            acc1 = _mm256_fmadd_ps(v_x1, v_w1, acc1);
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

pub fn vec_dot_q4_k_scalar(x: &[f32], block: &[u8]) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let dmin = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));

    let scales = &block[4..16];
    let qs = &block[16..144];

    let get_scale_min = |is: usize| -> (f32, f32) {
        let sc: u8;
        let m: u8;
        if is < 4 {
            sc = scales[is] & 63;
            m = scales[is + 4] & 63;
        } else {
            sc = (scales[is + 4] & 0xF) | ((scales[is - 4] >> 6) << 4);
            m = (scales[is + 4] >> 4) | ((scales[is] >> 6) << 4);
        }
        (d * (sc as f32), dmin * (m as f32))
    };

    let mut sum = 0.0f32;

    for j in 0..4 {
        let (sc0, m0) = get_scale_min(2 * j);
        let (sc1, m1) = get_scale_min(2 * j + 1);

        let q_offset = j * 32;
        let x_offset0 = j * 64;
        let x_offset1 = x_offset0 + 32;

        let x0 = &x[x_offset0..x_offset0 + 32];
        let x1 = &x[x_offset1..x_offset1 + 32];

        for l in 0..32 {
            let q_byte = qs[q_offset + l];
            let q_low = (q_byte & 0x0F) as f32;
            let q_high = (q_byte >> 4) as f32;

            sum += x0[l] * (sc0 * q_low - m0);
            sum += x1[l] * (sc1 * q_high - m1);
        }
    }

    sum
}

pub fn dequantize_q4_k(data: &[u8], num_elements: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; num_elements];
    let blocks = num_elements / 256;
    let block_size_bytes = 144;

    for b in 0..blocks {
        let offset = b * block_size_bytes;
        if offset + block_size_bytes > data.len() {
            break;
        }

        let block = &data[offset..offset + block_size_bytes];
        let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
        let dmin = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));

        let scales = &block[4..16];
        let qs = &block[16..144];

        let out_offset = b * 256;

        let get_scale_min = |is: usize| -> (f32, f32) {
            let sc: u8;
            let m: u8;
            if is < 4 {
                sc = scales[is] & 63;
                m = scales[is + 4] & 63;
            } else {
                sc = (scales[is + 4] & 0xF) | ((scales[is - 4] >> 6) << 4);
                m = (scales[is + 4] >> 4) | ((scales[is] >> 6) << 4);
            }
            (d * (sc as f32), dmin * (m as f32))
        };

        for j in 0..4 {
            let (sc0, m0) = get_scale_min(2 * j);
            let (sc1, m1) = get_scale_min(2 * j + 1);

            let q_offset = j * 32;
            let out0 = out_offset + j * 64;

            for l in 0..32 {
                let q_byte = qs[q_offset + l];

                let q_low = (q_byte & 0x0F) as f32;
                let q_high = (q_byte >> 4) as f32;

                if out0 + l < output.len() {
                    output[out0 + l] = sc0 * q_low - m0;
                }
                if out0 + l + 32 < output.len() {
                    output[out0 + l + 32] = sc1 * q_high - m1;
                }
            }
        }
    }

    output
}
