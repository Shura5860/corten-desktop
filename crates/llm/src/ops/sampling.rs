use rand::Rng;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct SamplerParams {
    pub temperature: f32,
    pub top_k: usize,
    pub top_p: f32,
    pub repetition_penalty: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            temperature: 0.6,
            top_k: 40,
            top_p: 0.9,
            repetition_penalty: 1.1,
        }
    }
}

pub fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

pub fn softmax(logits: &mut [f32]) {
    let max_val = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for val in logits.iter_mut() {
        *val = (*val - max_val).exp();
        sum += *val;
    }
    if sum > 0.0 {
        for val in logits.iter_mut() {
            *val /= sum;
        }
    }
}

pub fn apply_repetition_penalty(logits: &mut [f32], generated_tokens: &[usize], penalty: f32) {
    if penalty == 1.0 || generated_tokens.is_empty() {
        return;
    }
    let penalty_window = 64;
    let start_window = generated_tokens.len().saturating_sub(penalty_window);
    let recent_unique_tokens: HashSet<usize> = generated_tokens[start_window..].iter().cloned().collect();

    for &prev_token in &recent_unique_tokens {
        if prev_token < logits.len() {
            if logits[prev_token] > 0.0 {
                logits[prev_token] /= penalty;
            } else {
                logits[prev_token] *= penalty;
            }
        }
    }
}

pub fn sample_top_p(logits: &mut [f32], temperature: f32, top_p: f32, stop_token_ids: &HashSet<usize>) -> usize {
    let params = SamplerParams {
        temperature,
        top_k: 40,
        top_p,
        repetition_penalty: 1.1,
    };
    sample_token(logits, &params, stop_token_ids)
}

pub fn sample_token(
    logits: &mut [f32],
    params: &SamplerParams,
    stop_token_ids: &HashSet<usize>,
) -> usize {
    for &stop_id in stop_token_ids {
        if stop_id < logits.len() {
            logits[stop_id] = f32::NEG_INFINITY;
        }
    }

    if params.temperature > 0.0 {
        for val in logits.iter_mut() {
            if *val != f32::NEG_INFINITY {
                *val /= params.temperature;
            }
        }
    }

    let mut indexed: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
    indexed.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let k_cutoff = params.top_k.min(indexed.len());
    indexed.truncate(k_cutoff);

    let max_logit = indexed.iter().map(|(_, val)| *val).fold(f32::NEG_INFINITY, f32::max);
    let mut sum_probs = 0.0f32;
    let mut probs: Vec<(usize, f32)> = indexed
        .into_iter()
        .map(|(idx, val)| {
            let p = if val == f32::NEG_INFINITY { 0.0 } else { (val - max_logit).exp() };
            sum_probs += p;
            (idx, p)
        })
        .collect();

    if sum_probs > 0.0 {
        for (_, p) in probs.iter_mut() {
            *p /= sum_probs;
        }
    }

    let mut cumulative_p = 0.0;
    let mut cutoff_index = probs.len();
    for (i, &(_, p)) in probs.iter().enumerate() {
        cumulative_p += p;
        if cumulative_p >= params.top_p {
            cutoff_index = i + 1;
            break;
        }
    }

    let filtered_probs = &probs[..cutoff_index];

    let mut rng = rand::thread_rng();
    let r: f32 = rng.gen();
    let mut current_p = 0.0;

    for &(idx, p) in filtered_probs {
        current_p += p;
        if r <= current_p {
            return idx;
        }
    }

    filtered_probs.first().map(|&(idx, _)| idx).unwrap_or(1)
}
