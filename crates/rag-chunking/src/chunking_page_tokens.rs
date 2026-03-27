//! Page-aware token chunker.
//!
//! Splits on form-feed page separators, then token-chunks each page and
//! annotates chunks with `[page N chunk M]` labels.

use crate::chunking_token::chunk_text_tokens;

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
        let mut page_chunks = chunk_text_tokens(trimmed, max_tokens, overlap_ratio);
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
