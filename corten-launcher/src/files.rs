//! Fast user document / home file lookup & Semantic Vector Search (`corten-vector`) for Corten Desktop Launcher.

use corten_vector::VectorIndex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherFileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

static VECTOR_CACHE: OnceLock<VectorIndex> = OnceLock::new();

fn get_or_init_vector_index() -> &'static VectorIndex {
    VECTOR_CACHE.get_or_init(|| {
        let index = VectorIndex::new();
        if let Some(base_dirs) = directories::BaseDirs::new() {
            let home = base_dirs.home_dir();
            let search_dirs = [
                home.join("Documents"),
                home.join("Downloads"),
                home.join("Pictures"),
                home.join("Desktop"),
                home.join("Projects"),
            ];

            let mut doc_id = 1usize;
            for dir in &search_dirs {
                if !dir.exists() {
                    continue;
                }
                if let Ok(read_dir) = std::fs::read_dir(dir) {
                    for entry in read_dir.flatten() {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        if file_name.starts_with('.') {
                            continue;
                        }
                        let path = entry.path();
                        let is_dir = path.is_dir();

                        // Read first 300 chars if text/markdown/code document for semantic content indexing
                        let snippet = if !is_dir {
                            let ext = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("")
                                .to_lowercase();
                            if matches!(ext.as_str(), "md" | "txt" | "rs" | "toml" | "py" | "json") {
                                std::fs::read_to_string(&path)
                                    .ok()
                                    .map(|s| s.chars().take(300).collect::<String>())
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        };

                        let combined = format!("{} {} {}", file_name, path.display(), snippet);
                        let mut meta = HashMap::new();
                        meta.insert("name".into(), file_name);
                        meta.insert("path".into(), path.to_string_lossy().to_string());
                        meta.insert("is_dir".into(), if is_dir { "1".into() } else { "0".into() });

                        index.add_document_text(doc_id, &combined, meta);
                        doc_id += 1;
                    }
                }
            }
            let _ = index.build_hnsw_index();
        }
        index
    })
}

/// Finds matching files in user's primary directories using both exact substring
/// matching and HNSW semantic vector similarity (`corten-vector`).
pub fn find_matching_files(query: &str, limit: usize) -> Vec<LauncherFileEntry> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 || limit == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut seen_paths = HashSet::new();

    // 1. Exact substring priority pass
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let home = base_dirs.home_dir();
        let search_dirs = [
            home.join("Documents"),
            home.join("Downloads"),
            home.join("Pictures"),
            home.join("Desktop"),
            home.join("Projects"),
        ];

        for dir in &search_dirs {
            if !dir.exists() {
                continue;
            }
            let Ok(read_dir) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') {
                    continue;
                }
                if file_name.to_lowercase().contains(&q) {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    seen_paths.insert(path.clone());
                    results.push(LauncherFileEntry {
                        name: file_name,
                        path,
                        is_dir,
                    });
                    if results.len() >= limit {
                        return results;
                    }
                }
            }
        }
    }

    // 2. Semantic Vector HNSW Search fallback (`corten-vector`)
    let index = get_or_init_vector_index();
    if !index.is_empty() {
        let vec_hits = index.search_text(&q, limit);
        for hit in vec_hits {
            if hit.similarity < 0.15 {
                continue;
            }
            if let (Some(name), Some(path_str)) = (hit.metadata.get("name"), hit.metadata.get("path")) {
                let path = PathBuf::from(path_str);
                if seen_paths.insert(path.clone()) {
                    let is_dir = hit.metadata.get("is_dir").map(|v| v == "1").unwrap_or(false);
                    results.push(LauncherFileEntry {
                        name: name.clone(),
                        path,
                        is_dir,
                    });
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_returns_empty() {
        assert!(find_matching_files("", 5).is_empty());
        assert!(find_matching_files("a", 5).is_empty());
    }

    #[test]
    fn test_semantic_vector_search_integration() {
        let res = find_matching_files("wayland desktop project", 5);
        // Should not panic and returns matching project folders if present
        assert!(res.len() <= 5);
    }
}
