//! Context assembly for LLM prompts.
//!
//! Transforms ranked retrieval results into a token-budget-aware context
//! string with optional citations and deduplication.

use std::sync::{Arc, LazyLock};

use tiktoken_rs::CoreBPE;

use crate::fusion::FusedChunk;

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[allow(clippy::disallowed_methods)] // LazyLock init requires expect
static CL100K_TOKENIZER: LazyLock<Arc<CoreBPE>> = LazyLock::new(|| {
    Arc::new(tiktoken_rs::cl100k_base().expect("cl100k_base tokenizer must load"))
});

fn count_tokens(tokenizer: &CoreBPE, text: &str) -> usize {
    tokenizer.encode_ordinary(text).len()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Deduplication strategy for context assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupeStrategy {
    /// Keep one chunk per document_id (highest-scored survives).
    ByDocId,
    /// Keep one chunk per chunk_id (different chunks from same doc kept).
    ByChunkId,
    /// No deduplication.
    None,
}

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub max_tokens: usize,
    pub max_chunks: usize,
    pub dedupe_strategy: DedupeStrategy,
    pub include_citations: bool,
}

#[derive(Debug, Clone)]
pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextChunk {
    pub chunk_id: String,
    pub text: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub fused_score: f32,
    pub token_count: usize,
    pub citation: Option<Citation>,
}

#[derive(Debug, Clone)]
pub struct ContextResult {
    /// Assembled context string ready for LLM prompt.
    pub text: String,
    /// Structured access to surviving chunks.
    pub chunks: Vec<ContextChunk>,
    /// Top-level citation list (flattened from per-chunk citations).
    pub citations: Vec<Citation>,
    pub stats: ContextStats,
}

#[derive(Debug, Clone, Default)]
pub struct ContextStats {
    pub input_count: usize,
    pub after_dedupe: usize,
    pub final_count: usize,
    pub dedupe_dropped: usize,
    pub budget_dropped: usize,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Assembles fused retrieval results into LLM-ready context.
pub struct ContextBuilder {
    tokenizer: Arc<CoreBPE>,
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextBuilder {
    /// Create a builder with the default `cl100k_base` tokenizer.
    pub fn new() -> Self {
        Self { tokenizer: Arc::clone(&CL100K_TOKENIZER) }
    }

    /// Create a builder with a custom tokenizer.
    pub fn with_tokenizer(tokenizer: Arc<CoreBPE>) -> Self {
        Self { tokenizer }
    }

    /// Assemble context from fused chunks.
    ///
    /// Pipeline: sort -> deduplicate -> token budget truncation -> citations -> text assembly.
    pub fn build(&self, chunks: Vec<FusedChunk>, config: &ContextConfig) -> ContextResult {
        let input_count = chunks.len();

        // Step 1: Sort by fused_score desc, chunk_id asc.
        // This is defensive: callers are not required to pass pre-sorted input,
        // even though `rrf_fusion` already emits this ordering.
        let mut sorted = chunks;
        sorted.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });

        // Step 2: Deduplicate.
        let mut seen = std::collections::HashSet::new();
        let mut dedupe_dropped = 0usize;
        let deduped: Vec<FusedChunk> = sorted
            .into_iter()
            .filter(|chunk| {
                let key = match config.dedupe_strategy {
                    DedupeStrategy::ByDocId => Some(chunk.document_id.clone()),
                    DedupeStrategy::ByChunkId => Some(chunk.chunk_id.clone()),
                    DedupeStrategy::None => return true,
                };
                if let Some(k) = key {
                    if seen.insert(k) {
                        true
                    } else {
                        dedupe_dropped += 1;
                        false
                    }
                } else {
                    true
                }
            })
            .collect();

        let after_dedupe = deduped.len();

        // Step 3: Token budget truncation.
        let mut total_tokens = 0usize;
        let mut budget_dropped = 0usize;
        let mut final_chunks: Vec<ContextChunk> = Vec::new();

        for chunk in deduped {
            if final_chunks.len() >= config.max_chunks {
                budget_dropped += 1;
                continue;
            }
            let token_count = count_tokens(&self.tokenizer, &chunk.text);
            if total_tokens + token_count > config.max_tokens {
                budget_dropped += 1;
                continue;
            }

            total_tokens += token_count;

            let citation = if config.include_citations {
                Some(Citation {
                    chunk_id: chunk.chunk_id.clone(),
                    document_id: chunk.document_id.clone(),
                    chunk_index: chunk.chunk_index,
                    sources: chunk.sources.clone(),
                })
            } else {
                None
            };

            final_chunks.push(ContextChunk {
                chunk_id: chunk.chunk_id,
                text: chunk.text,
                document_id: chunk.document_id,
                chunk_index: chunk.chunk_index,
                fused_score: chunk.fused_score,
                token_count,
                citation,
            });
        }

        // Step 4: Build top-level citations.
        let citations: Vec<Citation> =
            final_chunks.iter().filter_map(|c| c.citation.clone()).collect();

        // Step 5: Assemble context string.
        let text = final_chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n\n");

        let final_count = final_chunks.len();

        ContextResult {
            text,
            chunks: final_chunks,
            citations,
            stats: ContextStats {
                input_count,
                after_dedupe,
                final_count,
                dedupe_dropped,
                budget_dropped,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::fusion::FusedChunk;

    fn fused(id: &str, doc: &str, index: i32, score: f32, text: &str) -> FusedChunk {
        FusedChunk {
            chunk_id: id.to_string(),
            document_id: doc.to_string(),
            chunk_index: index,
            text: text.to_string(),
            fused_score: score,
            sources: vec!["dense".to_string()],
            source_scores: HashMap::from([("dense".to_string(), score)]),
        }
    }

    fn default_config() -> ContextConfig {
        ContextConfig {
            max_tokens: 10_000,
            max_chunks: 50,
            dedupe_strategy: DedupeStrategy::None,
            include_citations: false,
        }
    }

    #[test]
    fn by_doc_id_keeps_highest_scored_per_document() {
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, "first chunk of doc1"),
            fused("c2", "doc1", 1, 0.5, "second chunk of doc1"),
            fused("c3", "doc2", 0, 0.7, "first chunk of doc2"),
        ];
        let config = ContextConfig { dedupe_strategy: DedupeStrategy::ByDocId, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.stats.input_count, 3);
        assert_eq!(result.stats.after_dedupe, 2);
        assert_eq!(result.stats.dedupe_dropped, 1);
        assert_eq!(result.chunks.len(), 2);
        let ids: Vec<&str> = result.chunks.iter().map(|c| c.document_id.as_str()).collect();
        assert!(ids.contains(&"doc1"));
        assert!(ids.contains(&"doc2"));
    }

    #[test]
    fn by_chunk_id_allows_multiple_chunks_from_same_doc() {
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, "first chunk"),
            fused("c1", "doc1", 0, 0.5, "duplicate c1"),
            fused("c2", "doc1", 1, 0.7, "second chunk same doc"),
        ];
        let config =
            ContextConfig { dedupe_strategy: DedupeStrategy::ByChunkId, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.stats.dedupe_dropped, 1);
        assert_eq!(result.chunks.len(), 2);
    }

    #[test]
    fn none_dedupe_keeps_all() {
        let chunks =
            vec![fused("c1", "doc1", 0, 0.9, "first"), fused("c2", "doc1", 1, 0.5, "second")];
        let result = ContextBuilder::new().build(chunks, &default_config());
        assert_eq!(result.stats.dedupe_dropped, 0);
        assert_eq!(result.chunks.len(), 2);
    }

    #[test]
    fn token_budget_truncates() {
        let big_text = "word ".repeat(500);
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, &big_text),
            fused("c2", "doc1", 1, 0.8, &big_text),
            fused("c3", "doc2", 0, 0.7, &big_text),
        ];
        let config = ContextConfig { max_tokens: 1100, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert!(result.stats.final_count <= 2);
        assert!(result.stats.budget_dropped >= 1);
    }

    #[test]
    fn first_chunk_exceeding_remaining_budget_is_dropped() {
        let small = "hello world";
        let big = "word ".repeat(5000);
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, small),
            fused("c2", "doc2", 0, 0.8, &big),
            fused("c3", "doc3", 0, 0.7, small),
        ];
        let config = ContextConfig { max_tokens: 100, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.stats.budget_dropped, 1);
        let ids: Vec<&str> = result.chunks.iter().map(|c| c.chunk_id.as_str()).collect();
        assert!(ids.contains(&"c1"));
        assert!(!ids.contains(&"c2"));
        assert!(ids.contains(&"c3"));
    }

    #[test]
    fn max_chunks_limit_enforced() {
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, "a"),
            fused("c2", "doc2", 0, 0.8, "b"),
            fused("c3", "doc3", 0, 0.7, "c"),
        ];
        let config = ContextConfig { max_chunks: 2, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.stats.final_count, 2);
        assert_eq!(result.stats.budget_dropped, 1);
    }

    #[test]
    fn citations_included_when_enabled() {
        let chunks = vec![fused("c1", "doc1", 0, 0.9, "text")];
        let config = ContextConfig { include_citations: true, ..default_config() };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].chunk_id, "c1");
        assert_eq!(result.citations[0].document_id, "doc1");
        assert_eq!(result.citations[0].chunk_index, 0);
    }

    #[test]
    fn citations_absent_when_disabled() {
        let chunks = vec![fused("c1", "doc1", 0, 0.9, "text")];
        let result = ContextBuilder::new().build(chunks, &default_config());
        assert!(result.citations.is_empty());
    }

    #[test]
    fn stats_are_accurate() {
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, "hello"),
            fused("c2", "doc1", 1, 0.8, "world"),
            fused("c3", "doc2", 0, 0.7, "foo"),
        ];
        let config = ContextConfig {
            max_tokens: 10_000,
            max_chunks: 50,
            dedupe_strategy: DedupeStrategy::ByDocId,
            include_citations: false,
        };
        let result = ContextBuilder::new().build(chunks, &config);
        assert_eq!(result.stats.input_count, 3);
        assert_eq!(result.stats.after_dedupe, 2);
        assert_eq!(result.stats.final_count, 2);
        assert_eq!(result.stats.dedupe_dropped, 1);
        assert_eq!(result.stats.budget_dropped, 0);
    }

    #[test]
    fn assembled_text_joins_with_double_newline() {
        let chunks =
            vec![fused("c1", "doc1", 0, 0.9, "first"), fused("c2", "doc2", 0, 0.8, "second")];
        let result = ContextBuilder::new().build(chunks, &default_config());
        assert_eq!(result.text, "first\n\nsecond");
    }
}
