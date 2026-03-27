//! Markdown heading-aware chunker.
//!
//! Splits text on markdown headings (`# ` through `###### `) and emits
//! [`ChunkWithSection`] values carrying section hierarchy metadata.

use crate::{
    CHARS_PER_TOKEN, ChunkWithSection, SectionInfo, chunking_sentences::chunk_text_sentences,
    chunking_token::chunk_text_tokens,
};

/// Markdown-aware chunker returning text only (drops section metadata).
pub fn chunk_text_markdown(text: &str, max_tokens: usize, overlap_ratio: f32) -> Vec<String> {
    chunk_text_markdown_sectioned(text, max_tokens, overlap_ratio)
        .into_iter()
        .map(|c| c.text)
        .collect()
}

/// Returns whether heading detection is enabled for a sidecar value.
///
/// `None` and unknown values default to enabled. Common disabled values:
/// `off`, `false`, `0`, `disabled` (case-insensitive).
pub fn is_heading_detection_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None => true,
        Some(mode) => {
            !mode.eq_ignore_ascii_case("off")
                && !mode.eq_ignore_ascii_case("false")
                && !mode.eq_ignore_ascii_case("disabled")
                && mode != "0"
        }
    }
}

/// Returns true when text contains markdown heading lines (`#` through `######`).
pub(crate) fn contains_markdown_headings(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("#### ")
            || trimmed.starts_with("##### ")
            || trimmed.starts_with("###### ")
    })
}

/// Parse a markdown heading line into `(level, title)`.
fn parse_heading(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let next = trimmed.chars().nth(level)?;
    if !next.is_whitespace() {
        return None;
    }
    let title = trimmed[level..].trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some((level as u8, title))
}

/// Build a hierarchical section path from a heading stack.
fn build_section_path(stack: &[(u8, String)]) -> Option<String> {
    if stack.is_empty() {
        return None;
    }
    let mut path = String::new();
    for (i, (_, title)) in stack.iter().enumerate() {
        if i > 0 {
            path.push_str(" > ");
        }
        path.push_str(title);
    }
    Some(path)
}

fn markdown_sections_with_info(text: &str) -> Vec<(String, SectionInfo)> {
    let normalized = if text.contains("\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    let mut sections = Vec::with_capacity(8);
    let mut current = String::new();
    let mut stack: Vec<(u8, String)> = Vec::with_capacity(6);
    let mut current_section = SectionInfo::default();

    for line in normalized.lines() {
        if let Some((level, title)) = parse_heading(line) {
            if !current.trim().is_empty() {
                sections.push((current.trim().to_string(), current_section.clone()));
                current.clear();
            }

            while let Some((existing_level, _)) = stack.last() {
                if *existing_level >= level {
                    stack.pop();
                } else {
                    break;
                }
            }
            stack.push((level, title.clone()));

            current_section = SectionInfo {
                section_title: Some(title),
                section_path: build_section_path(&stack),
                heading_level: Some(level),
            };
        }

        current.push_str(line);
        current.push('\n');
    }

    if !current.trim().is_empty() {
        sections.push((current.trim().to_string(), current_section));
    }

    // Legacy fallback: split on "\n#" when heading syntax is imperfect.
    const MAX_LEGACY_SECTIONS: usize = 2048;
    if sections.len() <= 1 && normalized.contains("\n#") {
        let split: Vec<String> = normalized
            .split("\n#")
            .take(MAX_LEGACY_SECTIONS)
            .enumerate()
            .map(|(i, seg)| if i == 0 { seg.to_string() } else { format!("#{}", seg) })
            .filter(|s| !s.trim().is_empty())
            .collect();

        if split.len() > 1 {
            return split
                .into_iter()
                .map(|section| {
                    let first = section.lines().next().unwrap_or_default();
                    let info = parse_heading(first)
                        .map(|(level, title)| SectionInfo {
                            section_title: Some(title.clone()),
                            section_path: Some(title),
                            heading_level: Some(level),
                        })
                        .unwrap_or_default();
                    (section, info)
                })
                .collect();
        }
    }

    sections
}

/// Markdown-aware chunker with section hierarchy metadata.
pub fn chunk_text_markdown_sectioned(
    text: &str,
    max_tokens: usize,
    overlap_ratio: f32,
) -> Vec<ChunkWithSection> {
    if max_tokens == 0 {
        return vec![ChunkWithSection {
            text: text.trim().to_string(),
            section: SectionInfo::default(),
        }];
    }

    let overlap_ratio = overlap_ratio.clamp(0.0, 0.9);
    let bpe = crate::bpe();
    let sections = markdown_sections_with_info(text);

    if sections.is_empty() {
        return chunk_text_tokens(text, max_tokens, overlap_ratio)
            .into_iter()
            .map(|text| ChunkWithSection { text, section: SectionInfo::default() })
            .collect();
    }

    if sections.len() == 1 && sections[0].1 == SectionInfo::default() {
        return chunk_text_sentences(&sections[0].0, max_tokens, overlap_ratio)
            .into_iter()
            .map(|text| ChunkWithSection { text, section: SectionInfo::default() })
            .collect();
    }

    let mut chunks = Vec::new();
    for (section, info) in sections {
        let token_len = bpe
            .map(|bpe| bpe.encode_with_special_tokens(&section).len())
            .unwrap_or_else(|| ((section.len() as f32) / CHARS_PER_TOKEN).ceil() as usize);

        if token_len <= max_tokens {
            chunks.push(ChunkWithSection { text: section.trim().to_string(), section: info });
            continue;
        }

        let mut carry_heading = info.heading_level.is_some();
        let heading = if carry_heading {
            section.lines().next().unwrap_or_default().to_string()
        } else {
            String::new()
        };
        if carry_heading && parse_heading(&heading).is_none() {
            carry_heading = false;
        }
        let body_without_heading = if carry_heading {
            section.find('\n').map(|pos| &section[pos + 1..]).unwrap_or("").to_string()
        } else {
            String::new()
        };
        let heading_tokens = if carry_heading {
            bpe.map(|bpe| bpe.encode_with_special_tokens(&heading).len())
                .unwrap_or_else(|| ((heading.len() as f32) / CHARS_PER_TOKEN).ceil() as usize)
        } else {
            0
        };
        if carry_heading && heading_tokens >= max_tokens && !body_without_heading.trim().is_empty()
        {
            carry_heading = false;
        }
        let body = if carry_heading { body_without_heading } else { section.clone() };

        if body.trim().is_empty() {
            chunks.push(ChunkWithSection { text: section.trim().to_string(), section: info });
            continue;
        }

        let body_max_tokens = if carry_heading {
            max_tokens.saturating_sub(heading_tokens).max(1)
        } else {
            max_tokens
        };
        let body_chunks = chunk_text_tokens(&body, body_max_tokens, overlap_ratio);
        for body_chunk in body_chunks {
            let combined = if carry_heading && !heading.is_empty() {
                format!("{heading}\n{body_chunk}")
            } else {
                body_chunk
            };
            chunks.push(ChunkWithSection {
                text: combined.trim().to_string(),
                section: info.clone(),
            });
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_heading_h1() {
        assert_eq!(parse_heading("# Authentication"), Some((1, "Authentication".to_string())));
    }

    #[test]
    fn parse_heading_h6() {
        assert_eq!(parse_heading("###### Deep"), Some((6, "Deep".to_string())));
    }

    #[test]
    fn parse_heading_not_heading() {
        assert_eq!(parse_heading("No heading"), None);
        assert_eq!(parse_heading("###NoSpace"), None);
        assert_eq!(parse_heading("####### TooDeep"), None);
    }

    #[test]
    fn is_heading_detection_enabled_falsey_values() {
        assert!(is_heading_detection_enabled(None));
        assert!(!is_heading_detection_enabled(Some("off")));
        assert!(!is_heading_detection_enabled(Some(" false ")));
        assert!(!is_heading_detection_enabled(Some("0")));
        assert!(!is_heading_detection_enabled(Some("disabled")));
        assert!(is_heading_detection_enabled(Some("auto")));
    }

    #[test]
    fn contains_markdown_headings_detects_valid_headings() {
        assert!(contains_markdown_headings("# Title\nbody"));
        assert!(contains_markdown_headings("text\n### Section"));
        assert!(!contains_markdown_headings("plain text\nnot a heading"));
        assert!(!contains_markdown_headings("###NoSpace"));
    }

    #[test]
    fn build_section_path_empty() {
        assert_eq!(build_section_path(&[]), None);
    }

    #[test]
    fn build_section_path_nested() {
        let stack = vec![
            (1, "Chapter 1".to_string()),
            (2, "1.2 Security".to_string()),
            (3, "Authentication".to_string()),
        ];
        assert_eq!(
            build_section_path(&stack),
            Some("Chapter 1 > 1.2 Security > Authentication".to_string())
        );
    }

    #[test]
    fn markdown_sectioned_basic() {
        let text = "# Chapter 1\nIntro\n## 1.1 Overview\nDetails";
        let chunks = chunk_text_markdown_sectioned(text, 200, 0.0);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].section.section_path.as_deref(), Some("Chapter 1"));
        assert_eq!(chunks[1].section.section_path.as_deref(), Some("Chapter 1 > 1.1 Overview"));
    }

    #[test]
    fn markdown_sectioned_sibling_headings() {
        let text = "# H1\nIntro\n## A\nBody A\n## B\nBody B";
        let chunks = chunk_text_markdown_sectioned(text, 200, 0.0);
        assert!(chunks.iter().any(|c| c.section.section_path.as_deref() == Some("H1 > A")
            && c.section.section_title.as_deref() == Some("A")));
        assert!(chunks.iter().any(|c| c.section.section_path.as_deref() == Some("H1 > B")
            && c.section.section_title.as_deref() == Some("B")));
    }

    #[test]
    fn markdown_sectioned_level_jump() {
        let text = "# Root\nTop\n### Deep\nLeaf";
        let chunks = chunk_text_markdown_sectioned(text, 200, 0.0);
        assert!(chunks.iter().any(|c| c.section.section_path.as_deref() == Some("Root > Deep")
            && c.section.heading_level == Some(3)));
    }

    #[test]
    fn markdown_sectioned_large_section_inherits() {
        let mut text = String::from("# Section\n");
        text.push_str(&"token ".repeat(2000));
        let chunks = chunk_text_markdown_sectioned(&text, 80, 0.0);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.section.section_path.as_deref() == Some("Section")
            && c.section.section_title.as_deref() == Some("Section")));
    }

    #[test]
    fn markdown_sectioned_no_headings() {
        let text = "This is plain text without markdown headings. It should still chunk.";
        let chunks = chunk_text_markdown_sectioned(text, 20, 0.0);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.section == SectionInfo::default()));
    }

    #[test]
    fn markdown_sectioned_large_preamble_keeps_first_line() {
        let text = format!(
            "KEEP_ME_FIRST_LINE\n{}\n# Heading\nBody",
            "Long preamble content for splitting. ".repeat(400)
        );
        let chunks = chunk_text_markdown_sectioned(&text, 30, 0.0);
        let preamble_chunks: Vec<&ChunkWithSection> =
            chunks.iter().filter(|c| c.section == SectionInfo::default()).collect();
        assert!(!preamble_chunks.is_empty());
        assert!(preamble_chunks.iter().any(|c| c.text.contains("KEEP_ME_FIRST_LINE")));
    }

    #[test]
    fn markdown_sectioned_heading_carry_respects_max_tokens() {
        let text = format!(
            "# Heading\n{}",
            "This section body has enough tokens to force splitting. ".repeat(80)
        );
        let max_tokens = 40;
        let chunks = chunk_text_markdown_sectioned(&text, max_tokens, 0.15);
        assert!(chunks.len() > 1);

        let Some(bpe) = tiktoken_rs::get_bpe_from_model("cl100k_base").ok() else {
            return;
        };
        for chunk in chunks {
            let token_len = bpe.encode_with_special_tokens(&chunk.text).len();
            assert!(token_len <= max_tokens, "chunk exceeded max_tokens: {token_len}");
        }
    }

    #[test]
    fn markdown_sectioned_limits_heading_prefix_when_budget_exceeded() {
        let long_heading = format!("# {}", "VeryLongHeadingToken ".repeat(80));
        let text = format!("{long_heading}\n{}", "Body content ".repeat(60));
        let chunks = chunk_text_markdown_sectioned(&text, 30, 0.0);
        assert!(!chunks.is_empty());
        let heading_prefixed =
            chunks.iter().filter(|c| c.text.trim_start().starts_with('#')).count();
        assert!(heading_prefixed <= 1);
    }

    #[test]
    fn markdown_sectioned_keeps_heading_when_heading_only_and_over_budget() {
        let text = format!("# {}", "VeryLongHeadingToken ".repeat(120));
        let chunks = chunk_text_markdown_sectioned(&text, 30, 0.0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.trim_start().starts_with('#'));
    }

    #[test]
    fn splits_by_heading() {
        let text = "# H1\nPara1\n\n## H2\nPara2";
        let chunks = chunk_text_markdown(text, 100, 0.0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("# H1"));
        assert!(chunks[1].contains("## H2"));
    }

    #[test]
    fn markdown_handles_zero_max_tokens() {
        let text = "# H1\nPara1";
        let chunks = chunk_text_markdown(text, 0, 0.5);
        assert_eq!(chunks, vec![text.trim().to_string()]);
    }

    #[test]
    fn markdown_legacy_fallback_caps_section_count() {
        let text: String = (0..5000).map(|i| format!("\n#Section{}\nBody {}", i, i)).collect();
        let sections = markdown_sections_with_info(&text);
        const MAX_LEGACY_SECTIONS: usize = 2048;
        assert!(
            sections.len() <= MAX_LEGACY_SECTIONS,
            "legacy fallback should cap sections at {}, got {}",
            MAX_LEGACY_SECTIONS,
            sections.len()
        );
    }
}
