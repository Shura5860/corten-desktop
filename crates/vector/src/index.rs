use crate::types::{embed_text, normalize_vector, VectorPoint};
use crate::wal::{append_wal, WalEntry};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, RwLock};

pub const DEFAULT_EMBEDDING_DIM: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: usize,
    pub distance: f32,
    pub similarity: f32,
    pub metadata: HashMap<String, String>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct InnerState {
    pub raw_vectors: Vec<VectorPoint>,
    pub ids: Vec<usize>,
    pub metadata_store: HashMap<usize, HashMap<String, String>>,
    pub inverted_index: HashMap<String, HashMap<String, HashSet<usize>>>,
    pub tombstone_ids: HashSet<usize>,
    /// Multi-neighbor small-world adjacency list (node index -> neighbor indices)
    pub hnsw_neighbors: Vec<Vec<usize>>,
}

#[derive(Clone, Default)]
pub struct VectorIndex {
    state: Arc<RwLock<InnerState>>,
    wal_path: Arc<RwLock<Option<String>>>,
}

impl VectorIndex {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(InnerState::default())),
            wal_path: Arc::new(RwLock::new(None)),
        }
    }

    pub fn enable_wal(&self, path: impl Into<String>) {
        if let Ok(mut wal_lock) = self.wal_path.write() {
            *wal_lock = Some(path.into());
        }
    }

    pub fn add_vector(
        &self,
        id: usize,
        vector: &[f32],
        metadata: HashMap<String, String>,
    ) {
        let normalized_vec = normalize_vector(vector);
        if let Ok(wal_lock) = self.wal_path.read() {
            append_wal(
                &wal_lock,
                &WalEntry::Insert {
                    id,
                    vector: normalized_vec.clone(),
                    metadata: metadata.clone(),
                },
            );
        }

        if let Ok(mut state) = self.state.write() {
            state.raw_vectors.push(VectorPoint(normalized_vec));
            state.ids.push(id);
            state.metadata_store.insert(id, metadata.clone());

            for (k, v) in &metadata {
                state
                    .inverted_index
                    .entry(k.clone())
                    .or_default()
                    .entry(v.clone())
                    .or_default()
                    .insert(id);
            }

            state.tombstone_ids.remove(&id);
        }
    }

    pub fn add_document_text(
        &self,
        id: usize,
        text: &str,
        metadata: HashMap<String, String>,
    ) {
        let vec = embed_text(text, DEFAULT_EMBEDDING_DIM);
        self.add_vector(id, &vec, metadata);
    }

    pub fn add_batch_with_metadata(
        &self,
        ids: &[usize],
        vectors: &[Vec<f32>],
        metadatas: &[HashMap<String, String>],
    ) {
        let Ok(mut state) = self.state.write() else {
            return;
        };
        let Ok(wal_lock) = self.wal_path.read() else {
            return;
        };

        for (i, &id) in ids.iter().enumerate() {
            if let Some(row) = vectors.get(i) {
                let normalized_vec = normalize_vector(row);
                let meta = metadatas.get(i).cloned().unwrap_or_default();

                append_wal(
                    &wal_lock,
                    &WalEntry::Insert {
                        id,
                        vector: normalized_vec.clone(),
                        metadata: meta.clone(),
                    },
                );

                state.raw_vectors.push(VectorPoint(normalized_vec));
                state.ids.push(id);
                state.metadata_store.insert(id, meta.clone());

                for (k, v) in &meta {
                    state
                        .inverted_index
                        .entry(k.clone())
                        .or_default()
                        .entry(v.clone())
                        .or_default()
                        .insert(id);
                }

                state.tombstone_ids.remove(&id);
            }
        }
    }

    /// Builds the Small-World Navigable Graph (`M = 16` nearest neighbors per node) using Rayon
    pub fn build_hnsw_index(&self) -> Result<(), String> {
        let mut state = self.state.write().map_err(|_| "Lock poisoned")?;

        if state.raw_vectors.is_empty() {
            return Err("İndekslenecek veri yok!".into());
        }

        let n = state.raw_vectors.len();
        let m_neighbors = 16.min(n.saturating_sub(1));
        let vectors = &state.raw_vectors;

        let neighbors: Vec<Vec<usize>> = (0..n)
            .into_par_iter()
            .map(|i| {
                let pt = &vectors[i];
                let mut dists: Vec<(usize, f32)> = (0..n)
                    .filter(|&j| j != i)
                    .map(|j| (j, pt.distance(&vectors[j])))
                    .collect();
                dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                dists.into_iter().take(m_neighbors).map(|(j, _)| j).collect()
            })
            .collect();

        state.hnsw_neighbors = neighbors;
        Ok(())
    }

    pub fn vacuum(&self) -> usize {
        let Ok(mut state) = self.state.write() else {
            return 0;
        };
        let tombstones = state.tombstone_ids.clone();

        if tombstones.is_empty() {
            return 0;
        }

        let mut new_raw_vectors = Vec::new();
        let mut new_ids = Vec::new();

        for (vec, &id) in state.raw_vectors.iter().zip(state.ids.iter()) {
            if !tombstones.contains(&id) {
                new_raw_vectors.push(vec.clone());
                new_ids.push(id);
            }
        }

        let purged_count = tombstones.len();

        state.raw_vectors = new_raw_vectors;
        state.ids = new_ids;
        state.tombstone_ids.clear();
        drop(state);

        let _ = self.build_hnsw_index();
        purged_count
    }

    pub fn delete_id(&self, id: usize) -> bool {
        if let Ok(wal_lock) = self.wal_path.read() {
            append_wal(&wal_lock, &WalEntry::Delete { id });
        }

        let Ok(mut state) = self.state.write() else {
            return false;
        };

        if let Some(meta) = state.metadata_store.remove(&id) {
            for (k, v) in meta {
                if let Some(val_map) = state.inverted_index.get_mut(&k) {
                    if let Some(id_set) = val_map.get_mut(&v) {
                        id_set.remove(&id);
                    }
                }
            }
            state.tombstone_ids.insert(id);
            true
        } else if state.ids.contains(&id) {
            state.tombstone_ids.insert(id);
            true
        } else {
            false
        }
    }

    pub fn update_id(
        &self,
        id: usize,
        new_vector: &[f32],
        new_metadata: HashMap<String, String>,
    ) -> Result<bool, String> {
        self.delete_id(id);
        self.add_vector(id, new_vector, new_metadata);
        let _ = self.build_hnsw_index();
        Ok(true)
    }

    pub fn search_text(&self, query_text: &str, top_k: usize) -> Vec<SearchResult> {
        let query_vec = embed_text(query_text, DEFAULT_EMBEDDING_DIM);
        self.search_filtered(&query_vec, top_k, None, None, None)
    }

    pub fn search_filtered(
        &self,
        query_slice: &[f32],
        top_k: usize,
        filter_key: Option<&str>,
        filter_value: Option<&str>,
        operator: Option<&str>,
    ) -> Vec<SearchResult> {
        let Ok(state) = self.state.read() else {
            return Vec::new();
        };
        if state.raw_vectors.is_empty() {
            return Vec::new();
        }

        let normalized_query = normalize_vector(query_slice);
        let query_point = VectorPoint(normalized_query);

        let op = operator.unwrap_or("==");
        let mut allowed_ids: Option<HashSet<usize>> = None;

        if let (Some(k), Some(v)) = (filter_key, filter_value) {
            if op == "==" {
                if let Some(val_map) = state.inverted_index.get(k) {
                    if let Some(id_set) = val_map.get(v) {
                        let valid: HashSet<usize> = id_set
                            .iter()
                            .copied()
                            .filter(|id| !state.tombstone_ids.contains(id))
                            .collect();
                        if valid.is_empty() {
                            return Vec::new();
                        }
                        allowed_ids = Some(valid);
                    } else {
                        return Vec::new();
                    }
                } else {
                    return Vec::new();
                }
            }
        }

        // Parallel Rayon evaluation of distances across active vectors
        let mut candidates: Vec<(usize, f32, f32)> = state
            .raw_vectors
            .par_iter()
            .zip(state.ids.par_iter())
            .filter_map(|(pt, &id)| {
                if state.tombstone_ids.contains(&id) {
                    return None;
                }
                if let Some(ref valid_set) = allowed_ids {
                    if !valid_set.contains(&id) {
                        return None;
                    }
                }
                let dist = query_point.distance(pt);
                let sim = query_point.cosine_similarity(pt);
                Some((id, dist, sim))
            })
            .collect();

        // Sort by distance ascending
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Deduplicate by ID (latest wins if updated before vacuum) and apply non-equality filters
        let mut seen_ids = HashSet::new();
        let mut filtered_results = Vec::new();
        let empty_meta = HashMap::new();

        for (id, distance, similarity) in candidates {
            if !seen_ids.insert(id) {
                continue;
            }

            let meta = state.metadata_store.get(&id).unwrap_or(&empty_meta);

            if let (Some(k), Some(v)) = (filter_key, filter_value) {
                if op != "==" {
                    if let Some(actual_val) = meta.get(k) {
                        if op == "!=" {
                            if actual_val == v {
                                continue;
                            }
                        } else if let (Ok(actual_num), Ok(target_num)) =
                            (actual_val.parse::<f64>(), v.parse::<f64>())
                        {
                            let is_match = match op {
                                ">" => actual_num > target_num,
                                "<" => actual_num < target_num,
                                ">=" => actual_num >= target_num,
                                "<=" => actual_num <= target_num,
                                _ => false,
                            };
                            if !is_match {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
            }

            filtered_results.push(SearchResult {
                id,
                distance,
                similarity,
                metadata: meta.clone(),
            });

            if filtered_results.len() >= top_k {
                break;
            }
        }

        filtered_results
    }

    pub fn save_to_disk(&self, file_path: &str) -> Result<(), String> {
        let state = self.state.read().map_err(|_| "Lock poisoned")?;

        let serialized = serde_json::to_vec(&*state).map_err(|e| e.to_string())?;
        let mut file = File::create(file_path).map_err(|e| e.to_string())?;
        file.write_all(&serialized).map_err(|e| e.to_string())?;

        if let Ok(wal_lock) = self.wal_path.read() {
            if let Some(ref path) = *wal_lock {
                let _ = File::create(path);
            }
        }

        Ok(())
    }

    pub fn load_from_disk(&self, file_path: &str) -> Result<(), String> {
        let mut state = self.state.write().map_err(|_| "Lock poisoned")?;

        if let Ok(mut file) = File::open(file_path) {
            let mut buffer = Vec::new();
            if file.read_to_end(&mut buffer).is_ok() {
                if let Ok(loaded) = serde_json::from_slice::<InnerState>(&buffer) {
                    *state = loaded;
                }
            }
        }

        // Replay WAL if configured
        if let Ok(wal_lock) = self.wal_path.read() {
            if let Some(ref path) = *wal_lock {
                if let Ok(wal_file) = File::open(path) {
                    let reader = BufReader::new(wal_file);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Ok(entry) = serde_json::from_str::<WalEntry>(&line) {
                            match entry {
                                WalEntry::Insert {
                                    id,
                                    vector,
                                    metadata,
                                } => {
                                    state.raw_vectors.push(VectorPoint(vector));
                                    state.ids.push(id);
                                    state.metadata_store.insert(id, metadata.clone());

                                    for (k, v) in &metadata {
                                        state
                                            .inverted_index
                                            .entry(k.clone())
                                            .or_default()
                                            .entry(v.clone())
                                            .or_default()
                                            .insert(id);
                                    }
                                    state.tombstone_ids.remove(&id);
                                }
                                WalEntry::Delete { id } => {
                                    if let Some(meta) = state.metadata_store.remove(&id) {
                                        for (k, v) in meta {
                                            if let Some(val_map) = state.inverted_index.get_mut(&k)
                                            {
                                                if let Some(id_set) = val_map.get_mut(&v) {
                                                    id_set.remove(&id);
                                                }
                                            }
                                        }
                                    }
                                    state.tombstone_ids.insert(id);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        let Ok(state) = self.state.read() else {
            return 0;
        };
        state.ids.len().saturating_sub(state.tombstone_ids.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

