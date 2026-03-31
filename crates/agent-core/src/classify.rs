//! Deterministic query classification heuristics.
//!
//! No LLM call — purely pattern-based. Good enough for the spike; a future
//! iteration can add an LLM-backed classifier behind a feature flag.

use crate::types::QueryType;

/// Classify a query as structured, unstructured, or mixed.
pub fn classify_query(query: &str) -> QueryType {
    let lower = query.to_ascii_lowercase();

    let has_structured = has_structured_indicators(&lower);
    let has_semantic = has_semantic_indicators(&lower);

    match (has_structured, has_semantic) {
        (true, true) => QueryType::Mixed,
        (true, false) => QueryType::Structured,
        _ => QueryType::Unstructured,
    }
}

fn has_structured_indicators(q: &str) -> bool {
    const SQL_PATTERNS: &[&str] = &[
        "select ",
        "where ",
        " from ",
        "count of",
        "show me all",
        "how many",
        "list all",
        "group by",
        "order by",
    ];
    const OPERATORS: &[char] = &['=', '>', '<'];

    SQL_PATTERNS.iter().any(|p| q.contains(p)) || q.chars().any(|c| OPERATORS.contains(&c))
}

fn has_semantic_indicators(q: &str) -> bool {
    const SEMANTIC_KEYWORDS: &[&str] = &[
        "what is",
        "what are",
        "explain",
        "describe",
        "how does",
        "how do",
        "why ",
        "about ",
        "tell me",
        "summarize",
        "overview",
    ];

    SEMANTIC_KEYWORDS.iter().any(|k| q.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_semantic_query() {
        assert_eq!(
            classify_query("What is retrieval-augmented generation?"),
            QueryType::Unstructured
        );
    }

    #[test]
    fn pure_structured_query() {
        assert_eq!(classify_query("select name from users where id = 5"), QueryType::Structured);
    }

    #[test]
    fn mixed_query() {
        assert_eq!(
            classify_query("explain why count of records where status = active"),
            QueryType::Mixed,
        );
    }

    #[test]
    fn no_indicators_defaults_to_unstructured() {
        assert_eq!(classify_query("hello world"), QueryType::Unstructured);
    }

    #[test]
    fn how_many_is_structured() {
        assert_eq!(classify_query("how many documents are there"), QueryType::Structured);
    }

    #[test]
    fn list_all_is_structured() {
        assert_eq!(classify_query("list all users"), QueryType::Structured);
    }
}
