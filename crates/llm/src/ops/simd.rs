use rayon::prelude::*;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline(always)]
pub fn dot_product_simd(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len();
    let mut sum = 0.0f32;
    let mut i = 0;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            unsafe {
                let mut acc0 = _mm256_setzero_ps();
                let mut acc1 = _mm256_setzero_ps();

                while i + 15 < len {
                    let va0 = _mm256_loadu_ps(a.as_ptr().add(i));
                    let vb0 = _mm256_loadu_ps(b.as_ptr().add(i));
                    acc0 = _mm256_fmadd_ps(va0, vb0, acc0);

                    let va1 = _mm256_loadu_ps(a.as_ptr().add(i + 8));
                    let vb1 = _mm256_loadu_ps(b.as_ptr().add(i + 8));
                    acc1 = _mm256_fmadd_ps(va1, vb1, acc1);

                    i += 16;
                }

                while i + 7 < len {
                    let va = _mm256_loadu_ps(a.as_ptr().add(i));
                    let vb = _mm256_loadu_ps(b.as_ptr().add(i));
                    acc0 = _mm256_fmadd_ps(va, vb, acc0);
                    i += 8;
                }

                let acc = _mm256_add_ps(acc0, acc1);
                let mut temp = [0.0f32; 8];
                _mm256_storeu_ps(temp.as_mut_ptr(), acc);
                sum += temp.iter().sum::<f32>();
            }
        }
    }

    while i < len {
        sum += a[i] * b[i];
        i += 1;
    }

    sum
}

#[inline]
pub fn mat_vec_mul_into(out: &mut [f32], matrix: &[f32], vector: &[f32], rows: usize, cols: usize) {
    out[..rows].par_iter_mut().enumerate().for_each(|(r, res_val)| {
        let row_offset = r * cols;
        if row_offset + cols <= matrix.len() {
            let row_slice = &matrix[row_offset..row_offset + cols];
            *res_val = dot_product_simd(row_slice, vector);
        }
    });
}

pub fn mat_vec_mul_parallel(matrix: &[f32], vector: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; rows];
    mat_vec_mul_into(&mut result, matrix, vector, rows, cols);
    result
}
