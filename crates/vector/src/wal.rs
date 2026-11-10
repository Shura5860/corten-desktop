use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WalEntry {
    Insert {
        id: usize,
        vector: Vec<f32>,
        metadata: HashMap<String, String>,
    },
    Delete {
        id: usize,
    },
}

pub fn append_wal(wal_path: &Option<String>, entry: &WalEntry) {
    if let Some(ref path) = *wal_path {
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
            let mut writer = BufWriter::new(file);
            if let Ok(serialized) = serde_json::to_string(entry) {
                let _ = writeln!(writer, "{}", serialized);
                let _ = writer.flush();
            }
        }
    }
}

