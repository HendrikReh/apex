//! Recursive character splitter.
//!
//! Tries a hierarchy of delimiters (`\n\n`, `\n`, `. `, ` `) before falling
//! back to the approximate character chunker.

use crate::{CHARS_PER_TOKEN, chunking_chars::chunk_text_chars};

const DELIMS: [&str; 4] = ["\n\n", "\n", ". ", " "];

/// Recursive character splitter using a delimiter hierarchy.
pub fn chunk_text_recursive_chars(
    text: &str,
    max_tokens: usize,
    overlap_ratio: f32,
) -> Vec<String> {
    let approx_chars = (max_tokens as f32 * CHARS_PER_TOKEN).max(400.0) as usize;
    let mut chunks = split_recursive(text, approx_chars, 0);
    if overlap_ratio > 0.0 && chunks.len() > 1 {
        let overlap_chars = ((approx_chars as f32) * overlap_ratio.clamp(0.0, 0.9)) as usize;
        let mut overlapped = Vec::new();
        for (i, ch) in chunks.iter().enumerate() {
            if i == 0 {
                overlapped.push(ch.clone());
                continue;
            }
            let prev = overlapped.last().cloned().unwrap_or_default();
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
        chunks = overlapped;
    }
    chunks
}

fn split_recursive(text: &str, approx_chars: usize, delim_idx: usize) -> Vec<String> {
    if text.len() <= approx_chars {
        return vec![text.trim().to_string()];
    }
    if delim_idx >= DELIMS.len() {
        return chunk_text_chars(text, approx_chars / 4, 0.0);
    }
    let delim = DELIMS[delim_idx];
    let parts: Vec<&str> = if delim.is_empty() { vec![text] } else { text.split(delim).collect() };
    if parts.len() <= 1 {
        return split_recursive(text, approx_chars, delim_idx + 1);
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        let seg = part.trim();
        if seg.is_empty() {
            continue;
        }
        let candidate = if current.is_empty() {
            seg.to_string()
        } else {
            format!("{}{}{}", current, delim, seg)
        };
        if candidate.len() > approx_chars && !current.is_empty() {
            if current.len() > approx_chars {
                chunks.extend(split_recursive(&current, approx_chars, delim_idx + 1));
            } else {
                chunks.push(current.trim().to_string());
            }
            current = seg.to_string();
        } else {
            current = candidate;
        }

        if i == parts.len() - 1 && !current.is_empty() {
            if current.len() > approx_chars {
                chunks.extend(split_recursive(&current, approx_chars, delim_idx + 1));
            } else {
                chunks.push(current.trim().to_string());
            }
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_text_recursively() {
        let text = "Para one sentence one. Para one sentence two. Para one sentence three.\n\n\
                    Para two sentence one. Para two sentence two. Para two sentence three.";
        let text = text.repeat(50);
        let chunks = chunk_text_recursive_chars(&text, 3, 0.0);
        assert!(chunks.len() >= 2);
    }
}
