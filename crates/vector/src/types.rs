use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorPoint(pub Vec<f32>);

impl VectorPoint {
    pub fn distance(&self, other: &Self) -> f32 {
        self.0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| {
                let diff = a - b;
                diff * diff
            })
            .sum()
    }

    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        self.0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| a * b)
            .sum()
    }
}

pub fn normalize_vector(vec: &[f32]) -> Vec<f32> {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        vec.to_vec()
    } else {
        vec.iter().map(|x| x / norm).collect()
    }
}

/// Generates a deterministic, normalized semantic feature vector (`dim` dimensions)
/// from arbitrary text using signed locality-sensitive hashing (LSH) over word tokens,
/// character bigrams, and character trigrams.
pub fn embed_text(text: &str, dim: usize) -> Vec<f32> {
    let dim = dim.max(16);
    let mut vec = vec![0.0f32; dim];
    let lower = text.to_lowercase();

    // 1. Word-level token projection (higher weight for semantic words)
    for word in lower.split(|c: char| !c.is_alphanumeric()) {
        if word.len() >= 2 {
            let h1 = fnv1a_hash(word.as_bytes(), 0x811c_9dc5);
            let h2 = fnv1a_hash(word.as_bytes(), 0x9e37_79b9);
            let idx = (h1 as usize) % dim;
            let sign = if (h2 & 1) == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign * 2.5;

            // Prefix stem (first 4 chars) for morphological matching (especially Turkish suffixes)
            let chars: Vec<char> = word.chars().collect();
            if chars.len() >= 4 {
                let stem: String = chars[..4].iter().collect();
                let hs1 = fnv1a_hash(stem.as_bytes(), 0x811c_9dc5);
                let hs2 = fnv1a_hash(stem.as_bytes(), 0x85eb_ca6b);
                let s_idx = (hs1 as usize) % dim;
                let s_sign = if (hs2 & 1) == 0 { 1.0 } else { -1.0 };
                vec[s_idx] += s_sign * 1.8;
            }
        }
    }

    // 2. Character trigram projection (captures fuzzy & sub-word similarity)
    let chars: Vec<char> = lower.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() >= 3 {
        for window in chars.windows(3) {
            let mut buf = [0u8; 12];
            let mut len = 0;
            for &ch in window {
                let mut tmp = [0u8; 4];
                let s = ch.encode_utf8(&mut tmp);
                buf[len..len + s.len()].copy_from_slice(s.as_bytes());
                len += s.len();
            }
            let h1 = fnv1a_hash(&buf[..len], 0x811c_9dc5);
            let h2 = fnv1a_hash(&buf[..len], 0xc2b2_ae35);
            let idx = (h1 as usize) % dim;
            let sign = if (h2 & 1) == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign * 1.0;
        }
    }

    normalize_vector(&vec)
}

fn fnv1a_hash(bytes: &[u8], seed: u32) -> u32 {
    let mut hash = seed;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

