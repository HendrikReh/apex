//! Paragraph-boundary aware chunker.
//!
//! Splits on blank lines to respect paragraph boundaries, then packs
//! paragraphs into token-limited chunks. Falls back to the token chunker
//! for oversized paragraphs.

use crate::{CHARS_PER_TOKEN, chunking_token::chunk_text_tokens};

/// Paragraph-first chunker: splits on blank lines, packs into token-limited chunks.
///
/// `overlap_ratio` is only applied when an individual paragraph is too large
/// and must fall back to the token chunker. Normal paragraph packing does not
/// currently duplicate trailing content across adjacent packed chunks.
pub fn chunk_text_paragraphs(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let bpe = crate::bpe();

    let normalized = if text.contains("\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    let paragraphs: Vec<String> =
        split_paragraphs(&normalized).into_iter().map(|p| normalize_list_blocks(&p)).collect();

    if paragraphs.is_empty() {
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }

    let count_tokens = |s: &str| -> usize {
        bpe.map(|bpe| bpe.encode_with_special_tokens(s).len())
            .unwrap_or_else(|| ((s.len() as f32) / CHARS_PER_TOKEN).ceil() as usize)
    };

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;

    for paragraph in paragraphs.iter() {
        let para_tokens = count_tokens(paragraph);
        if para_tokens > max_tokens {
            // Flush accumulator before emitting oversized paragraph sub-chunks.
            if !current.trim().is_empty() {
                chunks.push(current.trim().to_string());
                current.clear();
                current_tokens = 0;
            }
            let token_chunks = chunk_text_tokens(paragraph, max_tokens, overlap_ratio);
            chunks.extend(token_chunks);
            continue;
        }
        // Separator between packed paragraphs costs ~1 token.
        let sep_cost = if current.is_empty() { 0 } else { 1 };
        if current_tokens + para_tokens + sep_cost > max_tokens && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
            current_tokens = 0;
        }
        // Recompute after potential flush — first paragraph in a fresh
        // accumulator has no separator cost.
        let sep_cost = if current.is_empty() { 0 } else { 1 };
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph.trim());
        current_tokens += para_tokens + sep_cost;
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    chunks
}

/// Returns true if any line starts with a list marker (`- `, `* `, or `1. `).
fn has_list_markers(paragraph: &str) -> bool {
    paragraph.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || (trimmed.bytes().take_while(|b| b.is_ascii_digit()).count() > 0
                && trimmed.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with(". "))
    })
}

fn normalize_list_blocks(paragraph: &str) -> String {
    if has_list_markers(paragraph) {
        paragraph.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
    } else {
        paragraph.to_string()
    }
}

fn split_paragraphs(text: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                paras.push(current.trim().to_string());
                current.clear();
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        paras.push(current.trim().to_string());
    }
    paras
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraph_chunker_packs_small_paragraphs() {
        let text = "Para one.\nLine two.\n\nPara two continues here.";
        let chunks = chunk_text_paragraphs(text, 50, 0.0);
        // Both paragraphs fit within 50 tokens — they should be packed together.
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("Para one."));
        assert!(chunks[0].contains("Para two continues here."));
    }

    #[test]
    fn paragraph_chunker_splits_when_budget_exceeded() {
        let text = "Short.\n\nAnother short paragraph.\n\nYet another.";
        let chunks = chunk_text_paragraphs(text, 3, 0.0);
        // With a 3-token budget each paragraph must be its own chunk.
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn normalize_list_blocks_preserves_actual_lists() {
        assert_eq!(
            normalize_list_blocks("- item one\n- item two\n- item three"),
            "- item one\n- item two\n- item three"
        );
    }

    #[test]
    fn normalize_list_blocks_preserves_numbered_lists() {
        assert_eq!(
            normalize_list_blocks("1. first\n2. second\n3. third"),
            "1. first\n2. second\n3. third"
        );
    }

    #[test]
    fn normalize_list_blocks_preserves_asterisk_lists() {
        assert_eq!(normalize_list_blocks("* alpha\n* beta"), "* alpha\n* beta");
    }

    #[test]
    fn normalize_list_blocks_ignores_hyphens_in_text() {
        let text = "The state-of-the-art model from 2024-01-15 is available.";
        assert_eq!(normalize_list_blocks(text), text);
    }

    #[test]
    fn normalize_list_blocks_ignores_asterisks_in_text() {
        let text = "Use bold with **asterisks** in markdown.";
        assert_eq!(normalize_list_blocks(text), text);
    }

    #[test]
    fn has_list_markers_detects_dash_lists() {
        assert!(has_list_markers("- item"));
        assert!(has_list_markers("text\n  - indented item"));
        assert!(!has_list_markers("no-list-here"));
    }

    #[test]
    fn has_list_markers_detects_numbered_lists() {
        assert!(has_list_markers("1. first"));
        assert!(has_list_markers("12. twelfth"));
        assert!(!has_list_markers("2024-01-01 a date"));
    }

    #[test]
    fn normalize_list_blocks_trims_whitespace_in_lists() {
        assert_eq!(
            normalize_list_blocks("  - item one  \n  - item two  "),
            "- item one\n- item two"
        );
    }
}
