//! Sentence-boundary aware chunker.
//!
//! Splits text into sentences using Unicode segmentation, then packs
//! sentences into token-limited chunks. Supports multilingual sentence
//! terminators (CJK, Arabic, Hindi).

use crate::{CHARS_PER_TOKEN, chunking_token::chunk_text_tokens};
use unicode_segmentation::UnicodeSegmentation;

/// Sentence-first chunker: splits on sentences, then packs into token-limited chunks.
pub fn chunk_text_sentences(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let bpe = match crate::bpe() {
        Some(bpe) => bpe,
        None => return chunk_sentences_char_budget(text, max_tokens, overlap_ratio),
    };

    let sents = sentences(text);
    if sents.is_empty() {
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }

    let overlap = (overlap_ratio.clamp(0.0, 0.9) * max_tokens as f32) as usize;
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;

    for sentence in sents {
        let sentence_tokens = bpe.encode_with_special_tokens(&sentence).len();
        let separator_estimate = if current.is_empty() { 0 } else { 1 };
        let estimated_total = current_tokens + sentence_tokens + separator_estimate;

        if estimated_total > max_tokens && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
            if sentence_tokens > max_tokens {
                let token_chunks = chunk_text_tokens(&sentence, max_tokens, overlap_ratio);
                chunks.extend(token_chunks);
                current_tokens = 0;
            } else {
                current.push_str(&sentence);
                current_tokens = sentence_tokens;
            }
        } else if estimated_total > max_tokens {
            let token_chunks = chunk_text_tokens(&sentence, max_tokens, overlap_ratio);
            chunks.extend(token_chunks);
            current.clear();
            current_tokens = 0;
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&sentence);
            current_tokens = estimated_total;
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    if overlap > 0 && chunks.len() > 1 {
        let mut overlapped = Vec::new();
        for (i, ch) in chunks.iter().enumerate() {
            if i == 0 {
                overlapped.push(ch.clone());
                continue;
            }
            let prev = &chunks[i - 1];
            let prev_tokens = bpe.encode_with_special_tokens(prev);
            let keep = prev_tokens.iter().rev().take(overlap).rev().cloned().collect::<Vec<_>>();
            let keep_text = bpe.decode(keep).unwrap_or_default();
            let merged = format!("{keep_text} {ch}");
            overlapped.push(merged.trim().to_string());
        }
        overlapped
    } else {
        chunks
    }
}

/// Extract sentences from text using Unicode segmentation with fallback.
///
/// First attempts Unicode sentence boundaries. If that produces no results,
/// falls back to splitting on `.`, `!`, `?` and their CJK/Arabic/Hindi
/// equivalents.
///
/// # Examples
///
/// ```
/// use rag_chunking::sentences;
///
/// let result = sentences("First sentence. Second sentence! Third?");
/// assert!(result.len() >= 3);
/// ```
pub fn sentences(text: &str) -> Vec<String> {
    let mut sents: Vec<String> =
        text.unicode_sentences().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if sents.is_empty() {
        // CJK (。！？), Arabic (؟), Hindi (।) alongside ASCII terminators
        sents = text
            .split_inclusive([
                '.', '!', '?', '\u{3002}', '\u{FF01}', '\u{FF1F}', '\u{061F}', '\u{0964}',
            ])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
    }
    sents
}

fn chunk_sentences_char_budget(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    let approx_chars = (max_tokens as f32 * CHARS_PER_TOKEN).max(400.0) as usize;
    let overlap_chars = ((approx_chars as f32) * overlap_ratio.clamp(0.0, 0.9)) as usize;
    let sents = sentences(text);
    if sents.is_empty() {
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for sentence in sents {
        let rollback_len = current.len();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
        if current.len() > approx_chars && rollback_len > 0 {
            current.truncate(rollback_len);
            chunks.push(current.trim().to_string());
            current.clear();
            if sentence.len() > approx_chars {
                let sub = chunk_text_tokens(&sentence, max_tokens, overlap_ratio);
                chunks.extend(sub);
            } else {
                current.push_str(&sentence);
            }
        } else if current.len() > approx_chars {
            let sub = chunk_text_tokens(&sentence, max_tokens, overlap_ratio);
            chunks.extend(sub);
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    if overlap_chars > 0 && chunks.len() > 1 {
        let mut overlapped = Vec::new();
        for (i, ch) in chunks.iter().enumerate() {
            if i == 0 {
                overlapped.push(ch.clone());
                continue;
            }
            let prev = &chunks[i - 1];
            let keep: String = prev
                .chars()
                .rev()
                .take(overlap_chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let merged = format!("{keep} {ch}");
            overlapped.push(merged.trim().to_string());
        }
        overlapped
    } else {
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_chunker_respects_sentences() {
        let text = "First. Second! Third?";
        let chunks = chunk_text_sentences(text, 50, 0.0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("First."));
        assert!(chunks[0].contains("Second!"));
        assert!(chunks[0].contains("Third?"));
    }

    #[test]
    fn sentence_splitter_handles_cjk_terminators() {
        let text = "机器学习很重要\u{3002}深度学习是其子集\u{3002}";
        let result = sentences(text);
        assert!(result.len() >= 2, "CJK text with 。 should split, got {}", result.len());
    }

    #[test]
    fn sentence_splitter_handles_cjk_exclamation() {
        let text = "这很重要\u{FF01}必须注意\u{FF01}";
        let result = sentences(text);
        assert!(result.len() >= 2, "CJK text with ！ should split, got {}", result.len());
    }

    #[test]
    fn sentence_splitter_handles_arabic_question() {
        let text = "ما هو التعلم الآلي\u{061F} إنه مجال مهم!";
        let result = sentences(text);
        assert!(result.len() >= 2, "Arabic text with ؟ should split, got {}", result.len());
    }

    #[test]
    fn sentence_splitter_handles_hindi_danda() {
        let text = "यह महत्वपूर्ण है\u{0964} यह और भी महत्वपूर्ण है\u{0964}";
        let result = sentences(text);
        assert!(result.len() >= 2, "Hindi text with danda should split, got {}", result.len());
    }

    #[test]
    fn sentence_splitter_english_unchanged() {
        let text = "First sentence. Second sentence! Third?";
        let result = sentences(text);
        assert!(result.len() >= 3, "English text should split normally, got {}", result.len());
    }

    #[test]
    fn sentence_chunker_single_long_sentence() {
        let long = "word ".repeat(500);
        let text = format!("{}.", long.trim());
        let chunks = chunk_text_sentences(&text, 10, 0.0);
        assert!(chunks.len() > 1);
        let joined = chunks.join(" ");
        assert!(joined.contains("word"));
    }

    #[test]
    fn sentence_chunker_many_sentences() {
        let text: String = (0..500).map(|i| format!("Sentence number {} is here. ", i)).collect();
        let chunks = chunk_text_sentences(&text, 100, 0.0);
        assert!(chunks.len() > 1);
        for i in 0..500 {
            let needle = format!("Sentence number {}", i);
            assert!(chunks.iter().any(|c| c.contains(&needle)), "missing sentence {}", i);
        }
    }
}
