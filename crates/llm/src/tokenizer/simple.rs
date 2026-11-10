use std::collections::HashMap;
use crate::gguf::GgufValue;

pub struct SimpleTokenizer {
    pub id_to_token: HashMap<u32, String>,
    pub token_to_id: HashMap<String, u32>,
}

impl SimpleTokenizer {
    pub fn new(tokens: &[GgufValue]) -> Self {
        let mut id_to_token = HashMap::new();
        let mut token_to_id = HashMap::new();
        for (id, val) in tokens.iter().enumerate() {
            if let GgufValue::String(token_str) = val {
                id_to_token.insert(id as u32, token_str.clone());
                token_to_id.insert(token_str.clone(), id as u32);
            }
        }
        Self { id_to_token, token_to_id }
    }

    pub fn decode_tokens(&self, tokens: &[usize]) -> String {
        let mut bytes = Vec::new();
        for &id in tokens {
            let raw = self.id_to_token
                .get(&(id as u32))
                .cloned()
                .unwrap_or_else(|| "<unk>".to_string());

            if raw.starts_with("<0x") && raw.ends_with('>') && raw.len() == 6 {
                if let Ok(b) = u8::from_str_radix(&raw[3..5], 16) {
                    bytes.push(b);
                    continue;
                }
            }

            let s = raw
                .replace('\u{2581}', " ")
                .replace('Ġ', " ")
                .replace('Ċ', "\n")
                .replace('ĉ', "\t");
            bytes.extend_from_slice(s.as_bytes());
        }
        String::from_utf8_lossy(&bytes).to_string()
    }

    pub fn decode(&self, id: u32) -> String {
        self.decode_tokens(&[id as usize])
    }

    pub fn encode(&self, text: &str) -> Vec<usize> {
        let mut prompt_ids = Vec::new();
        let special_tokens = [
            "<|begin_of_text|>",
            "<|end_of_text|>",
            "<|start_header_id|>",
            "<|end_header_id|>",
            "<|eot_id|>",
            "<|im_start|>",
            "<|im_end|>",
            "<|endoftext|>",
        ];

        let uses_g_space = self.token_to_id.keys().any(|k| k.starts_with('Ġ'));
        let mut remaining = text;

        while !remaining.is_empty() {
            // 1. Check for special tokens
            let mut matched_special = false;
            for &st in &special_tokens {
                if remaining.starts_with(st) {
                    if let Some(&id) = self.token_to_id.get(st) {
                        prompt_ids.push(id as usize);
                        remaining = &remaining[st.len()..];
                        matched_special = true;
                        break;
                    }
                }
            }

            if matched_special {
                continue;
            }

            // Find distance to next special token or end of string
            let mut next_special_idx = remaining.len();
            for &st in &special_tokens {
                if let Some(pos) = remaining.find(st) {
                    if pos < next_special_idx {
                        next_special_idx = pos;
                    }
                }
            }

            let text_chunk = &remaining[..next_special_idx];
            remaining = &remaining[next_special_idx..];

            let formatted_chunk = if uses_g_space {
                text_chunk.replace(' ', "Ġ")
            } else {
                text_chunk.to_string()
            };

            let mut chunk_rem = formatted_chunk.as_str();

            while !chunk_rem.is_empty() {
                let mut matched = false;
                for len in (1..=chunk_rem.len()).rev() {
                    if !chunk_rem.is_char_boundary(len) {
                        continue;
                    }

                    let sub = &chunk_rem[..len];
                    if let Some(&id) = self.token_to_id.get(sub) {
                        prompt_ids.push(id as usize);
                        chunk_rem = &chunk_rem[len..];
                        matched = true;
                        break;
                    }
                }

                if !matched {
                    let first_char = chunk_rem.chars().next().unwrap();
                    let char_str = first_char.to_string();
                    if let Some(&id) = self.token_to_id.get(&char_str) {
                        prompt_ids.push(id as usize);
                    }
                    chunk_rem = &chunk_rem[first_char.len_utf8()..];
                }
            }
        }

        if prompt_ids.is_empty() {
            vec![1]
        } else {
            prompt_ids
        }
    }
}
