pub mod index;
pub mod types;
pub mod wal;

pub use index::{SearchResult, VectorIndex, DEFAULT_EMBEDDING_DIM};
pub use types::{embed_text, normalize_vector, VectorPoint};
pub use wal::WalEntry;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_vector_insert_and_search() {
        let index = VectorIndex::new();
        let mut meta1 = HashMap::new();
        meta1.insert("path".into(), "/home/user/docs/kira_sozlesmesi.pdf".into());
        meta1.insert("type".into(), "pdf".into());
        meta1.insert("size".into(), "1024".into());

        let mut meta2 = HashMap::new();
        meta2.insert("path".into(), "/home/user/src/wayland_compositor.rs".into());
        meta2.insert("type".into(), "rust".into());
        meta2.insert("size".into(), "4096".into());

        index.add_document_text(
            1,
            "Ev kira sözleşmesi kontrat maddeleri ve depozito şartları",
            meta1,
        );
        index.add_document_text(
            2,
            "Smithay Wayland compositor DRM KMS rendering shader pipeline",
            meta2,
        );

        assert_eq!(index.len(), 2);
        assert!(index.build_hnsw_index().is_ok());

        let results = index.search_text("kira kontratı", 2);
        assert!(!results.is_empty());
        assert_eq!(results[0].id, 1);

        let code_results = index.search_text("wayland drm shader", 2);
        assert!(!code_results.is_empty());
        assert_eq!(code_results[0].id, 2);
    }

    #[test]
    fn test_filtered_search_and_tombstones() {
        let index = VectorIndex::new();

        let mut m1 = HashMap::new();
        m1.insert("category".into(), "finance".into());
        m1.insert("score".into(), "90".into());

        let mut m2 = HashMap::new();
        m2.insert("category".into(), "code".into());
        m2.insert("score".into(), "40".into());

        index.add_vector(10, &[1.0, 0.0, 0.0], m1);
        index.add_vector(20, &[0.9, 0.1, 0.0], m2);

        let eq_res = index.search_filtered(&[1.0, 0.0, 0.0], 5, Some("category"), Some("code"), Some("=="));
        assert_eq!(eq_res.len(), 1);
        assert_eq!(eq_res[0].id, 20);

        let gt_res = index.search_filtered(&[1.0, 0.0, 0.0], 5, Some("score"), Some("50"), Some(">"));
        assert_eq!(gt_res.len(), 1);
        assert_eq!(gt_res[0].id, 10);

        // Delete ID 10 and vacuum
        assert!(index.delete_id(10));
        assert_eq!(index.len(), 1);
        let purged = index.vacuum();
        assert_eq!(purged, 1);
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn test_persistence_and_wal_replay() {
        let db_path = "/tmp/corten_test_vector_db.json";
        let wal_path = "/tmp/corten_test_vector.wal";
        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(wal_path);

        let index = VectorIndex::new();
        index.enable_wal(wal_path);

        let mut meta = HashMap::new();
        meta.insert("title".into(), "Corten Architecture".into());
        index.add_vector(42, &[0.0, 1.0, 0.0], meta);

        // Create second instance and replay from WAL before full save
        let index2 = VectorIndex::new();
        index2.enable_wal(wal_path);
        assert!(index2.load_from_disk(db_path).is_ok());
        assert_eq!(index2.len(), 1);

        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(wal_path);
    }
}

