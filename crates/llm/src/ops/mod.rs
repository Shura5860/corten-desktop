pub mod norm;
pub mod rope;
pub mod sampling;
pub mod simd;

pub use norm::{rms_norm, rms_norm_into};
pub use rope::apply_rope;
pub use sampling::{apply_repetition_penalty, sample_token, sample_top_p, SamplerParams, silu, softmax};
pub use simd::{dot_product_simd, mat_vec_mul_into, mat_vec_mul_parallel};
