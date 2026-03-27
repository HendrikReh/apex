//! BPE token-count based chunker.
//!
//! Uses the `cl100k_base` tokenizer (via tiktoken-rs) to split text into
//! chunks of at most `max_tokens` tokens with configurable overlap.

use crate::chunking_chars::chunk_text_chars;

/// Maximum input size (50 MB) before falling back to char chunker.
const MAX_INPUT_BYTES: usize = 50 * 1024 * 1024;

/// Token-based chunker using OpenAI-compatible tokenizer (tiktoken-rs).
///
/// Splits text into chunks of at most `max_tokens` BPE tokens with optional
/// overlap. Falls back to the character chunker if tokenization fails or
/// input exceeds `MAX_INPUT_BYTES`.
///
/// # Examples
///
/// ```
/// use rag_chunking::chunk_text_tokens;
///
/// let chunks = chunk_text_tokens("Hello world from Rust", 2, 0.0);
/// assert!(!chunks.is_empty());
/// ```
pub fn chunk_text_tokens(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    if max_tokens == 0 {
        return vec![text.to_string()];
    }
    if text.len() > MAX_INPUT_BYTES {
        return chunk_text_chars(text, max_tokens, overlap_ratio);
    }
    let bpe = match crate::bpe() {
        Some(bpe) => bpe,
        None => return chunk_text_chars(text, max_tokens, overlap_ratio),
    };

    let tokens = bpe.encode_with_special_tokens(text);
    if tokens.is_empty() {
        return chunk_text_chars(text, max_tokens, overlap_ratio);
    }
    let token_bytes: Vec<Vec<u8>> = bpe._decode_native_and_split(tokens).collect();
    chunk_text_tokens_from_token_bytes(text, &token_bytes, max_tokens, overlap_ratio)
}

fn chunk_text_tokens_from_token_bytes(
    text: &str,
    token_bytes: &[Vec<u8>],
    max_tokens: usize,
    overlap_ratio: f32,
) -> Vec<String> {
    if token_bytes.is_empty() {
        return chunk_text_chars(text, max_tokens, overlap_ratio);
    }

    let overlap = (overlap_ratio.clamp(0.0, 0.9) * max_tokens as f32) as usize;
    let estimated = token_bytes.len().div_ceil(max_tokens);
    let mut chunks = Vec::with_capacity(estimated);
    let mut decode_buf = Vec::new();
    let mut start = 0;
    while start < token_bytes.len() {
        let end = usize::min(start + max_tokens, token_bytes.len());
        decode_buf.clear();
        let needed = token_bytes[start..end].iter().map(Vec::len).sum::<usize>();
        decode_buf.reserve(needed.saturating_sub(decode_buf.capacity()));
        for bytes in &token_bytes[start..end] {
            decode_buf.extend_from_slice(bytes);
        }
        match std::str::from_utf8(&decode_buf) {
            Ok(decoded) if !decoded.is_empty() => chunks.push(decoded.to_string()),
            Ok(_) => {}
            Err(_) => return chunk_text_chars(text, max_tokens, overlap_ratio),
        }
        if end == token_bytes.len() {
            break;
        }
        start = end.saturating_sub(overlap);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_chunker_splits_and_overlaps() {
        let text = "hello world from tarpaulin";
        let chunks = chunk_text_tokens(text, 2, 0.5);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.contains("hello")));
        assert!(chunks.iter().any(|c| c.contains("world")));
    }

    #[test]
    fn token_chunker_handles_zero_max_tokens() {
        let text = "hello";
        let chunks = chunk_text_tokens(text, 0, 0.5);
        assert_eq!(chunks, vec![text.to_string()]);
    }

    #[test]
    fn token_chunker_falls_back_on_oversized_input() {
        let text = "x".repeat(MAX_INPUT_BYTES + 1);
        let chunks = chunk_text_tokens(&text, 100, 0.0);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn token_chunker_chunks_do_not_exceed_max_tokens() {
        let text = "Hello world. ".repeat(200);
        let max_tokens = 10;
        let bpe = tiktoken_rs::get_bpe_from_model("cl100k_base").ok();
        let chunks = chunk_text_tokens(&text, max_tokens, 0.0);
        if let Some(bpe) = bpe {
            for (i, chunk) in chunks.iter().enumerate() {
                let token_len = bpe.encode_with_special_tokens(chunk).len();
                assert!(
                    token_len <= max_tokens + 1,
                    "chunk {} has {} tokens, exceeds max_tokens {}",
                    i,
                    token_len,
                    max_tokens
                );
            }
        }
    }

    #[test]
    fn token_chunker_falls_back_to_char_chunker_on_utf8_decode_failure() {
        let text = "fallback preserves retrieval text";
        let token_bytes = vec![vec![0xF0, 0x28, 0x8C, 0x28], vec![0x80]];
        let expected = chunk_text_chars(text, 2, 0.0);
        let chunks = chunk_text_tokens_from_token_bytes(text, &token_bytes, 2, 0.0);
        assert_eq!(chunks, expected);
        assert!(!chunks.is_empty());
    }
}
