//! Page-aware token chunker.
//!
//! Splits on form-feed page separators, then token-chunks each page and
//! annotates chunks with `[page N chunk M]` labels.

use crate::chunking_token::chunk_text_tokens;

/// Estimate token cost of a page/chunk label so we can reserve budget.
fn label_token_overhead(label: &str) -> usize {
    crate::bpe()
        .map(|bpe| bpe.encode_with_special_tokens(label).len())
        .unwrap_or_else(|| ((label.len() as f32) / crate::CHARS_PER_TOKEN).ceil() as usize)
}

/// Page-aware token chunker with page and chunk number annotations.
pub fn chunk_text_page_tokens(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let pages: Vec<&str> = text.split('\u{0c}').collect();
    if pages.len() <= 1 {
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }

    let mut chunks = Vec::new();
    for (page_idx, page) in pages.iter().enumerate() {
        let trimmed = page.trim();
        if trimmed.is_empty() {
            continue;
        }
        // max_tokens == 0 means "do not split" — pass through to chunk_text_tokens
        // without subtracting label overhead, preserving the no-split contract.
        let content_budget = if max_tokens == 0 {
            0
        } else {
            // Use a 4-digit chunk number to cover any realistic page length.
            let sample_label = format!("[page {} chunk 9999]\n", page_idx + 1);
            let reserved = label_token_overhead(&sample_label);
            max_tokens.saturating_sub(reserved).max(1)
        };

        let mut page_chunks = chunk_text_tokens(trimmed, content_budget, overlap_ratio);
        for (chunk_idx, chunk) in page_chunks.iter_mut().enumerate() {
            *chunk = format!("[page {} chunk {}]\n{}", page_idx + 1, chunk_idx + 1, chunk);
        }
        chunks.append(&mut page_chunks);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_with_page_labels() {
        let text = "Page1 text here.\u{0c}Page2 text continues.";
        let chunks = chunk_text_page_tokens(text, 50, 0.0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("[page 1 chunk 1]"));
        assert!(chunks[1].starts_with("[page 2 chunk 1]"));
    }

    #[test]
    fn falls_back_when_no_pages() {
        let text = "Just one page without form feed.";
        let chunks = chunk_text_page_tokens(text, 5, 0.0);
        assert!(!chunks.is_empty());
    }
}
