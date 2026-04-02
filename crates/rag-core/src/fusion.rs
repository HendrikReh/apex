//! Reciprocal Rank Fusion (RRF) for combining retrieval results.
//!
//! Pure ranking logic with no IO. Merges results from multiple search
//! sources (e.g. dense + sparse) into a single ranked list.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub score: f32,
    pub score_type: String,
}

#[derive(Debug, Clone)]
pub struct FusedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub fused_score: f32,
    pub score_type: String,
    pub sources: Vec<String>,
    pub source_scores: HashMap<String, f32>,
}

/// Fuse results from multiple retrieval sources using Reciprocal Rank Fusion.
///
/// Formula: `score = sum(1 / (k + rank + 1))` for each source containing the chunk.
/// Rank is 0-indexed within each source's result list.
///
/// Results are sorted by fused_score descending, then chunk_id ascending for
/// deterministic tie-breaking.
///
/// # Payload identity assumption
///
/// All sources read from the same Qdrant points, so payload fields (document_id,
/// chunk_index, text) are identical for a given chunk_id. First occurrence wins.
pub fn rrf_fusion(results: &[(&str, Vec<RetrievedChunk>)], k: u32) -> Vec<FusedChunk> {
    let mut scores: HashMap<String, FusedChunk> = HashMap::new();

    for (source_name, chunks) in results {
        for (rank, chunk) in chunks.iter().enumerate() {
            let rrf_score = 1.0 / (k as f32 + rank as f32 + 1.0);

            let entry = scores.entry(chunk.chunk_id.clone()).or_insert_with(|| FusedChunk {
                chunk_id: chunk.chunk_id.clone(),
                document_id: chunk.document_id.clone(),
                chunk_index: chunk.chunk_index,
                text: chunk.text.clone(),
                title: chunk.title.clone(),
                source_url: chunk.source_url.clone(),
                source_domain: chunk.source_domain.clone(),
                language: chunk.language.clone(),
                tags: chunk.tags.clone(),
                section_heading: chunk.section_heading.clone(),
                collection: chunk.collection.clone(),
                fused_score: 0.0,
                score_type: "rrf_fused".to_string(),
                sources: Vec::new(),
                source_scores: HashMap::new(),
            });

            if entry.title.is_none() {
                entry.title = chunk.title.clone();
            }
            if entry.source_url.is_none() {
                entry.source_url = chunk.source_url.clone();
            }
            if entry.source_domain.is_none() {
                entry.source_domain = chunk.source_domain.clone();
            }
            if entry.language.is_none() {
                entry.language = chunk.language.clone();
            }
            if entry.tags.is_empty() && !chunk.tags.is_empty() {
                entry.tags = chunk.tags.clone();
            }
            if entry.section_heading.is_none() {
                entry.section_heading = chunk.section_heading.clone();
            }
            if entry.collection.is_none() {
                entry.collection = chunk.collection.clone();
            }
            entry.fused_score += rrf_score;
            entry.sources.push((*source_name).to_string());
            entry.source_scores.insert((*source_name).to_string(), chunk.score);
        }
    }

    let mut fused: Vec<FusedChunk> = scores.into_values().collect();
    fused.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, doc: &str, index: i32, score: f32) -> RetrievedChunk {
        RetrievedChunk {
            chunk_id: id.to_string(),
            document_id: doc.to_string(),
            chunk_index: index,
            text: format!("text of {id}"),
            title: Some(format!("title of {doc}")),
            source_url: Some(format!("https://example.com/{doc}")),
            source_domain: Some("example.com".to_string()),
            language: Some("en".to_string()),
            tags: vec!["test".to_string()],
            section_heading: Some("Section".to_string()),
            collection: Some("test_collection".to_string()),
            score,
            score_type: "dense".to_string(),
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn single_source_ranks_by_rrf_score() {
        let dense = vec![chunk("c1", "doc1", 0, 0.9), chunk("c2", "doc1", 1, 0.8)];
        let fused = rrf_fusion(&[("dense", dense)], 60);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].chunk_id, "c1");
        assert_eq!(fused[1].chunk_id, "c2");
        let expected_score_rank0 = 1.0 / 61.0;
        assert!((fused[0].fused_score - expected_score_rank0).abs() < 1e-6);
        let expected_score_rank1 = 1.0 / 62.0;
        assert!((fused[1].fused_score - expected_score_rank1).abs() < 1e-6);
        assert_eq!(fused[0].sources, vec!["dense"]);
        assert_eq!(fused[0].score_type, "rrf_fused");
        assert_eq!(fused[0].source_domain.as_deref(), Some("example.com"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn dual_source_chunk_in_both_scores_higher() {
        let dense = vec![chunk("c1", "doc1", 0, 0.9), chunk("c2", "doc1", 1, 0.8)];
        let sparse = vec![chunk("c3", "doc2", 0, 0.7), chunk("c1", "doc1", 0, 0.6)];
        let fused = rrf_fusion(&[("dense", dense), ("sparse", sparse)], 60);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].chunk_id, "c1");
        let expected_c1 = 1.0 / 61.0 + 1.0 / 62.0;
        assert!((fused[0].fused_score - expected_c1).abs() < 1e-6);
        assert_eq!(fused[0].sources.len(), 2);
        assert!(fused[0].source_scores.contains_key("dense"));
        assert!(fused[0].source_scores.contains_key("sparse"));
        assert_eq!(fused[0].tags, vec!["test"]);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn tie_breaking_by_chunk_id() {
        let dense = vec![chunk("b_chunk", "doc1", 0, 0.9)];
        let sparse = vec![chunk("a_chunk", "doc2", 0, 0.8)];
        let fused = rrf_fusion(&[("dense", dense), ("sparse", sparse)], 60);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].chunk_id, "a_chunk");
        assert_eq!(fused[1].chunk_id, "b_chunk");
    }

    #[test]
    fn empty_inputs() {
        let fused = rrf_fusion(&[], 60);
        assert!(fused.is_empty());
        let fused = rrf_fusion(&[("dense", vec![])], 60);
        assert!(fused.is_empty());
    }
}
