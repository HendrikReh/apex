//! BM25 sparse vector generation for documents and queries.
//!
//! This module intentionally keeps the tokenizer simple for MVP: lowercase plus
//! alphanumeric token extraction only. No stemming, stopwords, CJK handling, or
//! field weighting is applied here.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use bm25::{Embedder, EmbedderBuilder, Embedding, Tokenizer};

use crate::config::AppConfig;

const DEFAULT_QUERY_B: f32 = 0.3;

#[derive(Debug, Clone, PartialEq)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Config {
    pub k1: f32,
    pub b: f32,
    pub avgdl: f32,
}

impl Bm25Config {
    pub fn from_app_config(config: &AppConfig) -> Self {
        Self { k1: config.bm25_k1, b: config.bm25_b, avgdl: config.bm25_avgdl }
    }
}

#[derive(Debug, Clone, Default)]
struct CoreTokenizer;

impl Tokenizer for CoreTokenizer {
    fn tokenize(&self, input_text: &str) -> Vec<String> {
        input_text
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(|token| token.to_lowercase())
            .collect()
    }
}

pub struct Bm25Embedder {
    config: Bm25Config,
    query_b_default: f32,
    document_embedder: Embedder<u32, CoreTokenizer>,
}

impl Bm25Embedder {
    pub fn new(config: &Bm25Config) -> Result<Self> {
        Self::new_with_query_b(config, DEFAULT_QUERY_B)
    }

    pub fn from_app_config(config: &AppConfig) -> Result<Self> {
        Self::new_with_query_b(&Bm25Config::from_app_config(config), config.bm25_query_b)
    }

    pub fn new_with_query_b(config: &Bm25Config, query_b_default: f32) -> Result<Self> {
        validate_config(*config, query_b_default)?;

        let document_embedder = build_embedder(*config, config.b);

        Ok(Self { config: *config, query_b_default, document_embedder })
    }

    pub fn embed_document(&self, text: &str) -> SparseVector {
        to_sparse_vector(self.document_embedder.embed(text))
    }

    pub fn embed_query(&self, text: &str, query_b: Option<f32>) -> SparseVector {
        let query_embedder = build_embedder(self.config, query_b.unwrap_or(self.query_b_default));
        to_sparse_vector(query_embedder.embed(text))
    }
}

fn validate_config(config: Bm25Config, query_b_default: f32) -> Result<()> {
    if config.k1 < 0.0 {
        bail!("bm25 k1 must be non-negative");
    }
    if !(0.0..=1.0).contains(&config.b) {
        bail!("bm25 b must be between 0.0 and 1.0");
    }
    if config.avgdl < 0.0 {
        bail!("bm25 avgdl must be non-negative");
    }
    if !(0.0..=1.0).contains(&query_b_default) {
        bail!("bm25 query_b must be between 0.0 and 1.0");
    }

    Ok(())
}

fn build_embedder(config: Bm25Config, b: f32) -> Embedder<u32, CoreTokenizer> {
    EmbedderBuilder::<u32, CoreTokenizer>::with_avgdl(config.avgdl)
        .k1(config.k1)
        .b(b)
        .tokenizer(CoreTokenizer)
        .build()
}

fn to_sparse_vector(embedding: Embedding<u32>) -> SparseVector {
    let mut merged = BTreeMap::<u32, f32>::new();
    for token in embedding.0 {
        *merged.entry(token.index).or_insert(0.0) += token.value;
    }

    let (indices, values): (Vec<u32>, Vec<f32>) = merged.into_iter().unzip();
    SparseVector { indices, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn document_embeddings_are_canonicalized_and_sorted() {
        let embedder = Bm25Embedder::new(&Bm25Config { k1: 1.2, b: 0.75, avgdl: 300.0 })
            .expect("bm25 embedder should build");

        let sparse = embedder.embed_document("alpha beta alpha");

        assert_eq!(sparse.indices.len(), 2);
        assert_eq!(sparse.values.len(), 2);
        assert!(sparse.indices.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn duplicate_terms_preserve_raw_embedding_weight_when_canonicalized() {
        let config = Bm25Config { k1: 1.2, b: 0.75, avgdl: 300.0 };
        let embedder = Bm25Embedder::new(&config).expect("bm25 embedder should build");
        let raw_embedding = build_embedder(config, config.b).embed("alpha alpha");

        assert_eq!(raw_embedding.0.len(), 2, "expected repeated term postings from bm25 crate");
        assert!(
            raw_embedding
                .0
                .windows(2)
                .all(|pair| pair[0].index == pair[1].index && pair[0].value == pair[1].value),
            "expected duplicate postings to share one index/value"
        );

        let sparse = embedder.embed_document("alpha alpha");

        assert_eq!(sparse.indices.len(), 1);
        assert_eq!(sparse.values.len(), 1);
        assert_eq!(sparse.indices[0], raw_embedding.0[0].index);
        assert_eq!(sparse.values[0], raw_embedding.0.iter().map(|token| token.value).sum::<f32>());
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn query_b_override_changes_query_embedding() {
        let embedder = Bm25Embedder::new(&Bm25Config { k1: 1.2, b: 0.75, avgdl: 10.0 })
            .expect("bm25 embedder should build");

        let default_query = embedder.embed_query("alpha beta gamma", None);
        let low_b_query = embedder.embed_query("alpha beta gamma", Some(0.0));

        assert_eq!(default_query.indices, low_b_query.indices);
        assert_ne!(default_query.values, low_b_query.values);
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn tokenizer_is_core_only_lowercase_alnum() {
        let embedder = Bm25Embedder::new(&Bm25Config { k1: 1.2, b: 0.75, avgdl: 300.0 })
            .expect("bm25 embedder should build");

        let sparse_a = embedder.embed_document("Hello, WORLD!");
        let sparse_b = embedder.embed_document("hello world");

        assert_eq!(sparse_a, sparse_b);
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn tokenizer_lowercases_unicode_letters() {
        let embedder = Bm25Embedder::new(&Bm25Config { k1: 1.2, b: 0.75, avgdl: 300.0 })
            .expect("bm25 embedder should build");

        let sparse_a = embedder.embed_document("Äpfel");
        let sparse_b = embedder.embed_document("äpfel");

        assert_eq!(sparse_a, sparse_b);
    }

    #[test]
    fn invalid_query_b_is_rejected() {
        let err = match Bm25Embedder::new_with_query_b(
            &Bm25Config { k1: 1.2, b: 0.75, avgdl: 300.0 },
            1.5,
        ) {
            Ok(_) => panic!("invalid query_b should fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("query_b"));
    }
}
