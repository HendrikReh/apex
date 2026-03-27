//! Paragraph-boundary aware chunker.
//!
//! Splits on blank lines to respect paragraph boundaries, then packs
//! paragraphs into token-limited chunks. Falls back to the token chunker
//! for oversized paragraphs.

use crate::{CHARS_PER_TOKEN, chunking_token::chunk_text_tokens};

/// Paragraph-first chunker: splits on blank lines, packs into token-limited chunks.
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

    let mut chunks = Vec::new();
    for paragraph in paragraphs.iter() {
        let token_count = bpe
            .map(|bpe| bpe.encode_with_special_tokens(paragraph).len())
            .unwrap_or_else(|| ((paragraph.len() as f32) / CHARS_PER_TOKEN).ceil() as usize);
        if token_count > max_tokens {
            let token_chunks = chunk_text_tokens(paragraph, max_tokens, overlap_ratio);
            chunks.extend(token_chunks);
        } else {
            chunks.push(paragraph.trim().to_string());
        }
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
    fn paragraph_chunker_respects_blank_lines() {
        let text = "Para one.\nLine two.\n\nPara two continues here.";
        let chunks = chunk_text_paragraphs(text, 50, 0.0);
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
