//! Form-feed page-level chunker.
//!
//! Splits on form-feed (`\x0C`) characters inserted between PDF pages,
//! then token-chunks each page independently.

use crate::chunking_token::chunk_text_tokens;

/// Page-level chunker: split on form-feed markers and chunk each page.
pub fn chunk_text_pages(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let overlap_ratio = overlap_ratio.clamp(0.0, 0.9);
    let pages: Vec<&str> = text.split('\u{0c}').collect();

    if max_tokens == 0 {
        if pages.len() <= 1 {
            return vec![format!("[page 1]\n{}", text.trim())];
        }
        return pages
            .into_iter()
            .enumerate()
            .filter_map(|(idx, p)| {
                let t = p.trim();
                if t.is_empty() { None } else { Some(format!("[page {}]\n{}", idx + 1, t)) }
            })
            .collect();
    }

    if pages.len() <= 1 {
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }

    let mut chunks = Vec::new();
    for (page_idx, page) in pages.iter().enumerate() {
        let trimmed = page.trim();
        if trimmed.is_empty() {
            continue;
        }
        let page_num = page_idx + 1;
        let page_chunks = chunk_text_tokens(trimmed, max_tokens, overlap_ratio);
        for ch in page_chunks {
            chunks.push(format!("[page {}]\n{}", page_num, ch));
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_form_feed() {
        let text = "Page1 text.\u{0c}Page2 text.";
        let chunks = chunk_text_pages(text, 50, 0.0);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].starts_with("[page 1]\n"));
        assert!(chunks[0].contains("Page1"));
        assert!(chunks.last().unwrap().starts_with("[page 2]\n"));
        assert!(chunks.last().unwrap().contains("Page2"));
    }

    #[test]
    fn handles_zero_max_tokens() {
        let text = "Page1 text.\u{0c}Page2 text.";
        let chunks = chunk_text_pages(text, 0, 0.5);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("[page 1]"));
        assert!(chunks[1].starts_with("[page 2]"));
    }

    #[test]
    fn page_labels_use_source_page_number() {
        let text = "Short.\u{0c}This is a longer page with many words that should produce chunks.";
        let chunks = chunk_text_pages(text, 5, 0.0);
        for chunk in &chunks[1..] {
            assert!(
                chunk.starts_with("[page 2]\n"),
                "expected [page 2] label but got: {}",
                chunk.chars().take(20).collect::<String>()
            );
        }
    }

    #[test]
    fn labels_consistent_between_zero_and_normal_budget() {
        let text = "Page1.\u{0c}Page2.\u{0c}Page3.";
        let zero = chunk_text_pages(text, 0, 0.0);
        let normal = chunk_text_pages(text, 1000, 0.0);
        for (z, n) in zero.iter().zip(normal.iter()) {
            let z_label: String = z.lines().next().unwrap_or("").to_string();
            let n_label: String = n.lines().next().unwrap_or("").to_string();
            assert_eq!(z_label, n_label);
        }
    }
}
