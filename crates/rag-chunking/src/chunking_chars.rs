//! Approximate character-budget chunker.
//!
//! Uses a word buffer to split text into chunks sized by an estimated
//! character count derived from the token budget.

use crate::CHARS_PER_TOKEN;

/// Maximum input size (50 MB) before truncation.
const MAX_INPUT_BYTES: usize = 50 * 1024 * 1024;

fn cap_input_to_max_bytes(text: &str, max_input_bytes: usize) -> &str {
    if text.len() <= max_input_bytes {
        return text;
    }
    let mut end = max_input_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Approximate character-based chunker using a word buffer.
///
/// Token-to-character ratio is defined by [`CHARS_PER_TOKEN`].
pub fn chunk_text_chars(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let text = cap_input_to_max_bytes(text, MAX_INPUT_BYTES);
    let approx_chars = (max_tokens as f32 * CHARS_PER_TOKEN).max(400.0) as usize;
    let overlap_chars = ((approx_chars as f32) * overlap_ratio.clamp(0.0, 0.9)) as usize;

    let mut chunks = Vec::new();
    let mut buffer = String::new();

    for word in text.split_whitespace() {
        if buffer.len() + word.len() + 1 > approx_chars && !buffer.is_empty() {
            chunks.push(buffer.trim().to_string());
            if overlap_chars > 0 {
                let overlap = chunks
                    .last()
                    .map(|c: &String| c.chars().rev().take(overlap_chars).collect::<Vec<_>>())
                    .unwrap_or_default();
                let overlap: String = overlap.into_iter().rev().collect();
                buffer = overlap;
            } else {
                buffer.clear();
            }
        }
        buffer.push_str(word);
        buffer.push(' ');
    }

    if !buffer.trim().is_empty() {
        chunks.push(buffer.trim().to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_chunker_empty_input() {
        assert!(chunk_text_chars("", 100, 0.0).is_empty());
    }

    #[test]
    fn char_chunker_whitespace_only() {
        assert!(chunk_text_chars("   \n\t  ", 100, 0.0).is_empty());
    }

    #[test]
    fn char_chunker_single_short_text() {
        assert_eq!(chunk_text_chars("hello world", 100, 0.0), vec!["hello world"]);
    }

    #[test]
    fn char_chunker_splits_long_text() {
        let text = "word ".repeat(200);
        let chunks = chunk_text_chars(&text, 100, 0.0);
        assert!(chunks.len() > 1);
        let total_words: usize = chunks.iter().map(|c| c.split_whitespace().count()).sum();
        assert_eq!(total_words, 200);
    }

    #[test]
    fn char_chunker_overlap_produces_shared_content() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima \
                    mike november oscar papa quebec romeo sierra tango uniform victor whiskey \
                    xray yankee zulu "
            .repeat(3);
        let chunks_with = chunk_text_chars(&text, 100, 0.5);
        let chunks_without = chunk_text_chars(&text, 100, 0.0);
        assert!(chunks_with.len() > 1);
        let tail = &chunks_with[0][chunks_with[0].len().saturating_sub(20)..];
        assert!(chunks_with[1].contains(tail));
        assert!(chunks_with.len() >= chunks_without.len());
    }

    #[test]
    fn char_chunker_handles_cjk() {
        let text = "机器学习 深度学习 自然语言处理 计算机视觉";
        let chunks = chunk_text_chars(text, 100, 0.0);
        assert!(!chunks.is_empty());
        assert!(chunks[0].contains("机器学习"));
    }

    #[test]
    fn char_chunker_long_single_word() {
        let text = "a".repeat(2000);
        let chunks = chunk_text_chars(&text, 100, 0.0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2000);
    }

    #[test]
    fn char_chunker_caps_oversized_input() {
        let text = "x".repeat(MAX_INPUT_BYTES + 1);
        let chunks = chunk_text_chars(&text, 100, 0.0);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].len(), MAX_INPUT_BYTES);
    }

    #[test]
    fn char_chunker_cap_respects_utf8_boundaries() {
        let text = "é".repeat(16);
        let capped = cap_input_to_max_bytes(&text, 15);
        assert!(capped.is_char_boundary(capped.len()));
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
    }
}
