//! Text chunking strategies for RAG document ingestion.
//!
//! This crate provides multiple strategies for splitting text into chunks
//! suitable for embedding and retrieval in a RAG pipeline. Each strategy
//! trades off between split quality and computational cost.
//!
//! # Strategies
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | [`Tokens`](ChunkingStrategy::Tokens) | BPE token-count splits (default) |
//! | [`Sentences`](ChunkingStrategy::Sentences) | Sentence-aware packing |
//! | [`Markdown`](ChunkingStrategy::Markdown) | Heading-aware with section metadata |
//! | [`Paragraphs`](ChunkingStrategy::Paragraphs) | Blank-line paragraph splits |
//! | [`Chars`](ChunkingStrategy::Chars) | Approximate character-budget splits |
//! | [`Pages`](ChunkingStrategy::Pages) | Form-feed page splits |
//! | [`PageTokens`](ChunkingStrategy::PageTokens) | Page-aware token splits |
//! | [`RecursiveChars`](ChunkingStrategy::RecursiveChars) | Hierarchical delimiter splits |
//! | [`Semantic`](ChunkingStrategy::Semantic) | LLM-guided boundary detection |

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod chunking_chars;
mod chunking_markdown;
mod chunking_page_tokens;
mod chunking_pages;
mod chunking_paragraphs;
mod chunking_recursive;
mod chunking_semantic;
mod chunking_sentences;
mod chunking_token;
mod settings;

pub use chunking_chars::chunk_text_chars;
pub use chunking_markdown::{
    chunk_text_markdown, chunk_text_markdown_sectioned, is_heading_detection_enabled,
};
pub use chunking_page_tokens::chunk_text_page_tokens;
pub use chunking_pages::chunk_text_pages;
pub use chunking_paragraphs::chunk_text_paragraphs;
pub use chunking_recursive::chunk_text_recursive_chars;
pub use chunking_semantic::chunk_text_semantic;
pub use chunking_sentences::{chunk_text_sentences, sentences};
pub use chunking_token::chunk_text_tokens;
pub use settings::{ChunkingSettings, settings};

use std::{fmt::Display, str::FromStr, sync::OnceLock};

use tiktoken_rs::CoreBPE;

/// Returns a cached reference to the cl100k_base BPE tokenizer.
///
/// Initialized once on first call and reused for the process lifetime.
/// Returns `None` if the tokenizer fails to load.
pub(crate) fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

/// Approximate characters per token for cl100k_base.
///
/// Fallback when BPE tokenization is unavailable and for character-budget
/// estimation from token counts.
pub(crate) const CHARS_PER_TOKEN: f32 = 4.0;

/// Metadata about the document section a chunk belongs to.
///
/// Populated by the markdown chunker from heading hierarchy. Other strategies
/// return [`SectionInfo::default()`] (all fields `None`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SectionInfo {
    /// Immediate heading title, e.g. "Authentication".
    pub section_title: Option<String>,
    /// Full heading hierarchy, e.g. "Chapter 1 > 1.2 Security > Authentication".
    pub section_path: Option<String>,
    /// Heading level for markdown headings (1-6).
    pub heading_level: Option<u8>,
}

/// A text chunk paired with its section metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChunkWithSection {
    /// The chunk text content.
    pub text: String,
    /// Section hierarchy metadata (populated by markdown chunker).
    pub section: SectionInfo,
}

// ============================================================================
// Word Boundary Utilities
// ============================================================================

/// Returns true if the character belongs to a script with upper/lower case
/// distinctions (Latin, Greek, Cyrillic, etc.).
fn has_case(c: char) -> bool {
    c.is_uppercase() || c.is_lowercase()
}

/// Ensures a chunk starts at a word boundary by trimming any partial word.
///
/// If the text starts mid-word (e.g. "ystems in the AI Act"), finds the first
/// whitespace and trims everything before it.
///
/// For scripts without case distinctions (CJK, Arabic, Hebrew, etc.), no
/// trimming is applied since characters are self-contained units.
pub fn ensure_word_boundary_start(text: &str) -> &str {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return trimmed;
    }

    let Some(first_char) = trimmed.chars().next() else {
        return trimmed;
    };

    if has_case(first_char)
        && first_char.is_lowercase()
        && let Some(ws_pos) = trimmed.find(char::is_whitespace)
    {
        return trimmed[ws_pos..].trim_start();
    }

    trimmed
}

/// Post-processes chunks to ensure they start at word boundaries.
///
/// Fixes chunks that begin mid-word by trimming leading partial words.
/// Chunks that become empty after trimming are dropped.
///
/// # Examples
///
/// ```
/// use rag_chunking::normalize_chunk_boundaries;
///
/// let chunks = vec![
///     "ystems in the AI Act".to_string(),
///     "Valid start".to_string(),
/// ];
/// let fixed = normalize_chunk_boundaries(chunks);
/// assert_eq!(fixed[0], "in the AI Act");
/// assert_eq!(fixed[1], "Valid start");
/// ```
pub fn normalize_chunk_boundaries(chunks: Vec<String>) -> Vec<String> {
    chunks
        .into_iter()
        .filter_map(|chunk| {
            let normalized = ensure_word_boundary_start(&chunk);
            if normalized.is_empty() {
                return None;
            }
            if normalized.len() == chunk.len() { Some(chunk) } else { Some(normalized.to_string()) }
        })
        .collect()
}

/// Post-processes sectioned chunks to ensure chunk text starts at word boundaries.
pub fn normalize_chunk_boundaries_sectioned(
    chunks: Vec<ChunkWithSection>,
) -> Vec<ChunkWithSection> {
    chunks
        .into_iter()
        .filter_map(|chunk| {
            let normalized = ensure_word_boundary_start(&chunk.text);
            if normalized.is_empty() {
                return None;
            }
            if normalized.len() == chunk.text.len() {
                Some(chunk)
            } else {
                Some(ChunkWithSection { text: normalized.to_string(), section: chunk.section })
            }
        })
        .collect()
}

/// Chunking strategies supported across the workspace.
///
/// Parsed from sidecar configuration via [`FromStr`]. Case-insensitive.
///
/// # Examples
///
/// ```
/// use rag_chunking::ChunkingStrategy;
///
/// let strategy: ChunkingStrategy = "markdown".parse().unwrap();
/// assert_eq!(strategy, ChunkingStrategy::Markdown);
/// assert_eq!(strategy.as_str(), "markdown");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkingStrategy {
    /// BPE token-count based splitting (default).
    Tokens,
    /// Page-aware token splitting with page/chunk labels.
    PageTokens,
    /// Sentence-boundary aware packing.
    Sentences,
    /// LLM-guided semantic boundary detection.
    Semantic,
    /// Approximate character-budget splitting.
    Chars,
    /// Blank-line paragraph splitting.
    Paragraphs,
    /// Markdown heading-aware splitting with section metadata.
    Markdown,
    /// Form-feed page splitting with page labels.
    Pages,
    /// Hierarchical delimiter-based recursive splitting.
    RecursiveChars,
}

impl ChunkingStrategy {
    /// Returns the canonical string name for this strategy.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ChunkingStrategy::Tokens => "tokens",
            ChunkingStrategy::PageTokens => "page_tokens",
            ChunkingStrategy::Sentences => "sentences",
            ChunkingStrategy::Semantic => "semantic",
            ChunkingStrategy::Chars => "chars",
            ChunkingStrategy::Paragraphs => "paragraphs",
            ChunkingStrategy::Markdown => "markdown",
            ChunkingStrategy::Pages => "pages",
            ChunkingStrategy::RecursiveChars => "recursive_chars",
        }
    }

    /// Returns an array of all available strategies.
    pub const fn all() -> [ChunkingStrategy; 9] {
        [
            ChunkingStrategy::Tokens,
            ChunkingStrategy::PageTokens,
            ChunkingStrategy::Sentences,
            ChunkingStrategy::Semantic,
            ChunkingStrategy::Chars,
            ChunkingStrategy::Paragraphs,
            ChunkingStrategy::Markdown,
            ChunkingStrategy::Pages,
            ChunkingStrategy::RecursiveChars,
        ]
    }
}

impl Display for ChunkingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChunkingStrategy {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("tokens") {
            Ok(ChunkingStrategy::Tokens)
        } else if s.eq_ignore_ascii_case("page_tokens")
            || s.eq_ignore_ascii_case("page-aware")
            || s.eq_ignore_ascii_case("pageaware")
        {
            Ok(ChunkingStrategy::PageTokens)
        } else if s.eq_ignore_ascii_case("sentences") {
            Ok(ChunkingStrategy::Sentences)
        } else if s.eq_ignore_ascii_case("semantic") {
            Ok(ChunkingStrategy::Semantic)
        } else if s.eq_ignore_ascii_case("chars")
            || s.eq_ignore_ascii_case("characters")
            || s.eq_ignore_ascii_case("approx")
        {
            Ok(ChunkingStrategy::Chars)
        } else if s.eq_ignore_ascii_case("paragraphs") || s.eq_ignore_ascii_case("paragraph") {
            Ok(ChunkingStrategy::Paragraphs)
        } else if s.eq_ignore_ascii_case("markdown") || s.eq_ignore_ascii_case("md") {
            Ok(ChunkingStrategy::Markdown)
        } else if s.eq_ignore_ascii_case("pages") || s.eq_ignore_ascii_case("page") {
            Ok(ChunkingStrategy::Pages)
        } else if s.eq_ignore_ascii_case("recursive_chars") || s.eq_ignore_ascii_case("recursive") {
            Ok(ChunkingStrategy::RecursiveChars)
        } else {
            Err(format!(
                "unsupported chunking strategy '{}'; expected one of \
                 tokens|page_tokens|sentences|semantic|chars|paragraphs|markdown|pages|recursive_chars",
                s
            ))
        }
    }
}

/// Select the best chunking strategy for the given text.
///
/// If `configured_strategy` parses to a valid strategy, use it.
/// Otherwise auto-detect: markdown headings → `Markdown`, else `Tokens`.
pub fn select_strategy(
    configured_strategy: Option<&str>,
    text: &str,
    heading_detection: Option<&str>,
) -> ChunkingStrategy {
    if let Some(strategy) =
        configured_strategy.and_then(|s| ChunkingStrategy::from_str(s.trim()).ok())
    {
        return strategy;
    }
    if chunking_markdown::is_heading_detection_enabled(heading_detection)
        && chunking_markdown::contains_markdown_headings(text)
    {
        ChunkingStrategy::Markdown
    } else {
        ChunkingStrategy::Tokens
    }
}

/// Detect if text is predominantly CJK and adjust the token budget.
///
/// CJK characters consume 2-3 tokens each in cl100k_base, so a fixed token
/// budget produces chunks with less semantic content for CJK text. Applies
/// a multiplier when the dominant script is CJK.
#[allow(dead_code)]
pub(crate) fn adjust_tokens_for_script(
    text: &str,
    max_tokens: usize,
    cjk_multiplier: f32,
) -> usize {
    if cjk_multiplier <= 1.0 {
        return max_tokens;
    }

    let mut cjk_count = 0u32;
    let mut other_count = 0u32;
    for c in text.chars() {
        if is_cjk_char(c) {
            cjk_count += 1;
        } else if c.is_alphabetic() {
            other_count += 1;
        }
    }

    let total = cjk_count + other_count;
    if total == 0 {
        return max_tokens;
    }

    let cjk_ratio = cjk_count as f32 / total as f32;
    if cjk_ratio > 0.5 { (max_tokens as f32 * cjk_multiplier) as usize } else { max_tokens }
}

/// Check if a character is a CJK ideograph or kana.
///
/// # Examples
///
/// ```
/// use rag_chunking::is_cjk_char;
///
/// assert!(is_cjk_char('漢'));
/// assert!(is_cjk_char('あ'));
/// assert!(!is_cjk_char('A'));
/// ```
pub fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
    )
}

/// Dispatch chunking by strategy with word boundary normalization.
///
/// All strategies pass through `normalize_chunk_boundaries` because even
/// "natural boundary" strategies (sentences, paragraphs, markdown) can
/// internally delegate to the token chunker for oversized inputs or apply
/// overlap, producing chunks that start mid-word.
pub async fn chunk_text_with_strategy(
    text: &str,
    strategy: ChunkingStrategy,
    max_tokens: usize,
    overlap_ratio: f32,
    semantic_max_chars: usize,
) -> Vec<String> {
    let chunks = match strategy {
        ChunkingStrategy::Semantic => {
            chunk_text_semantic(text, max_tokens, overlap_ratio, semantic_max_chars).await
        }
        ChunkingStrategy::Tokens => chunk_text_tokens(text, max_tokens, overlap_ratio),
        ChunkingStrategy::PageTokens => chunk_text_page_tokens(text, max_tokens, overlap_ratio),
        ChunkingStrategy::Sentences => chunk_text_sentences(text, max_tokens, overlap_ratio),
        ChunkingStrategy::Chars => chunk_text_chars(text, max_tokens, overlap_ratio),
        ChunkingStrategy::Paragraphs => chunk_text_paragraphs(text, max_tokens, overlap_ratio),
        ChunkingStrategy::Markdown => chunk_text_markdown(text, max_tokens, overlap_ratio),
        ChunkingStrategy::Pages => chunk_text_pages(text, max_tokens, overlap_ratio),
        ChunkingStrategy::RecursiveChars => {
            chunk_text_recursive_chars(text, max_tokens, overlap_ratio)
        }
    };

    normalize_chunk_boundaries(chunks)
}

/// Dispatch chunking by strategy, returning chunks with section metadata.
///
/// Markdown chunking emits section metadata from heading hierarchy.
/// All other strategies emit default section metadata.
pub async fn chunk_text_with_strategy_sectioned(
    text: &str,
    strategy: ChunkingStrategy,
    max_tokens: usize,
    overlap_ratio: f32,
    semantic_max_chars: usize,
) -> Vec<ChunkWithSection> {
    match strategy {
        ChunkingStrategy::Markdown => normalize_chunk_boundaries_sectioned(
            chunk_text_markdown_sectioned(text, max_tokens, overlap_ratio),
        ),
        _ => {
            chunk_text_with_strategy(text, strategy, max_tokens, overlap_ratio, semantic_max_chars)
                .await
                .into_iter()
                .map(|text| ChunkWithSection { text, section: SectionInfo::default() })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strategy_names() {
        for name in [
            "tokens",
            "page_tokens",
            "sentences",
            "semantic",
            "chars",
            "paragraphs",
            "markdown",
            "pages",
            "recursive_chars",
        ] {
            let strat: ChunkingStrategy = name.parse().expect("strategy should parse");
            assert_eq!(strat.as_str(), name);
        }
    }

    #[tokio::test]
    async fn non_markdown_strategy_returns_default_section() {
        let chunks = chunk_text_with_strategy_sectioned(
            "alpha beta gamma delta epsilon zeta eta theta",
            ChunkingStrategy::Tokens,
            4,
            0.0,
            20_000,
        )
        .await;
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.section == SectionInfo::default()));
    }

    // ========================================================================
    // Strategy Selection
    // ========================================================================

    #[test]
    fn select_strategy_uses_configured_when_present() {
        let strategy = select_strategy(Some("sentences"), "# Heading\nBody", None);
        assert_eq!(strategy, ChunkingStrategy::Sentences);
    }

    #[test]
    fn select_strategy_auto_detects_markdown() {
        let strategy = select_strategy(None, "# Heading\nBody text", None);
        assert_eq!(strategy, ChunkingStrategy::Markdown);
    }

    #[test]
    fn select_strategy_defaults_to_tokens_without_headings() {
        let strategy = select_strategy(None, "Plain text without headings.", None);
        assert_eq!(strategy, ChunkingStrategy::Tokens);
    }

    #[test]
    fn select_strategy_respects_heading_detection_disabled() {
        let strategy = select_strategy(None, "# Heading\nBody", Some("off"));
        assert_eq!(strategy, ChunkingStrategy::Tokens);
    }

    // ========================================================================
    // Word Boundary
    // ========================================================================

    #[test]
    fn ensure_word_boundary_start_fixes_truncated_text() {
        assert_eq!(ensure_word_boundary_start("ystems in the AI Act"), "in the AI Act");
    }

    #[test]
    fn ensure_word_boundary_start_preserves_valid_text() {
        assert_eq!(ensure_word_boundary_start("Systems in the AI Act"), "Systems in the AI Act");
    }

    #[test]
    fn ensure_word_boundary_start_preserves_numbers() {
        assert_eq!(ensure_word_boundary_start("123 items found"), "123 items found");
    }

    #[test]
    fn ensure_word_boundary_start_preserves_punctuation() {
        assert_eq!(ensure_word_boundary_start("• Bullet point text"), "• Bullet point text");
    }

    #[test]
    fn ensure_word_boundary_start_handles_whitespace() {
        assert_eq!(ensure_word_boundary_start("   ystems in the AI Act"), "in the AI Act");
    }

    #[test]
    fn ensure_word_boundary_start_handles_empty() {
        assert_eq!(ensure_word_boundary_start(""), "");
        assert_eq!(ensure_word_boundary_start("   "), "");
    }

    #[test]
    fn ensure_word_boundary_start_handles_single_partial_word() {
        assert_eq!(ensure_word_boundary_start("ystems"), "ystems");
    }

    #[test]
    fn ensure_word_boundary_start_real_world_examples() {
        assert_eq!(ensure_word_boundary_start("ystems in the AI Act (Art."), "in the AI Act (Art.");
        assert_eq!(
            ensure_word_boundary_start("e risk analyses should be"),
            "risk analyses should be"
        );
        assert_eq!(
            ensure_word_boundary_start("ssary information.18 In certain"),
            "information.18 In certain"
        );
    }

    #[test]
    fn normalize_chunk_boundaries_fixes_all_chunks() {
        let chunks = vec![
            "ystems in the AI Act".to_string(),
            "Valid chunk starting correctly".to_string(),
            "artial word at start".to_string(),
        ];
        let normalized = normalize_chunk_boundaries(chunks);
        assert_eq!(normalized[0], "in the AI Act");
        assert_eq!(normalized[1], "Valid chunk starting correctly");
        assert_eq!(normalized[2], "word at start");
    }

    #[test]
    fn normalize_chunk_boundaries_filters_empty() {
        let chunks = vec![
            "partial".to_string(),
            "".to_string(),
            "   ".to_string(),
            "Valid text".to_string(),
        ];
        let normalized = normalize_chunk_boundaries(chunks);
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0], "partial");
        assert_eq!(normalized[1], "Valid text");
    }

    // ========================================================================
    // Unicode / Non-ASCII
    // ========================================================================

    #[test]
    fn ensure_word_boundary_start_handles_non_ascii() {
        assert_eq!(ensure_word_boundary_start("Über die Grenze"), "Über die Grenze");
        assert_eq!(ensure_word_boundary_start("ber die Grenze"), "die Grenze");
    }

    #[test]
    fn ensure_word_boundary_start_handles_proper_nouns() {
        // Lowercase-initial proper nouns like "iPhone" are treated as partial words.
        // Documented as expected (potentially lossy) behavior.
        assert_eq!(ensure_word_boundary_start("iPhone supports this"), "supports this");
    }

    #[test]
    fn ensure_word_boundary_start_handles_unicode_whitespace() {
        assert_eq!(ensure_word_boundary_start("artial\u{00A0}word test"), "word test");
        assert_eq!(ensure_word_boundary_start("ragment\u{2003}after space"), "after space");
    }

    #[test]
    fn ensure_word_boundary_start_preserves_cjk() {
        assert_eq!(
            ensure_word_boundary_start("机器学习是人工智能的子集"),
            "机器学习是人工智能的子集"
        );
        assert_eq!(ensure_word_boundary_start("プログラミング言語"), "プログラミング言語");
        assert_eq!(ensure_word_boundary_start("인공지능 기술"), "인공지능 기술");
    }

    #[test]
    fn ensure_word_boundary_start_preserves_arabic() {
        let arabic = "التعلم الآلي هو مجال";
        assert_eq!(ensure_word_boundary_start(arabic), arabic);
    }

    #[test]
    fn ensure_word_boundary_start_preserves_hebrew() {
        let hebrew = "למידת מכונה היא תחום";
        assert_eq!(ensure_word_boundary_start(hebrew), hebrew);
    }

    #[test]
    fn ensure_word_boundary_start_preserves_devanagari() {
        let hindi = "मशीन लर्निंग एक क्षेत्र है";
        assert_eq!(ensure_word_boundary_start(hindi), hindi);
    }

    #[test]
    fn ensure_word_boundary_start_preserves_thai() {
        let thai = "ภาษาไทยเป็นภาษาที่สวยงาม";
        assert_eq!(ensure_word_boundary_start(thai), thai);
    }

    #[test]
    fn normalize_chunk_boundaries_avoids_unnecessary_allocation() {
        let valid_chunk = "Valid chunk".to_string();
        let valid_chunk_ptr = valid_chunk.as_ptr();
        let chunks = vec![valid_chunk, "artial word here".to_string()];
        let normalized = normalize_chunk_boundaries(chunks);
        assert_eq!(normalized[0], "Valid chunk");
        assert_eq!(normalized[1], "word here");
        assert_eq!(normalized[0].as_ptr(), valid_chunk_ptr);
    }

    // ========================================================================
    // normalize_chunk_boundaries_sectioned
    // ========================================================================

    #[test]
    fn normalize_chunk_boundaries_sectioned_preserves_section_info() {
        let chunks = vec![
            ChunkWithSection {
                text: "Valid text".to_string(),
                section: SectionInfo {
                    section_title: Some("Title".to_string()),
                    section_path: Some("Root > Title".to_string()),
                    heading_level: Some(2),
                },
            },
            ChunkWithSection {
                text: "artial word here".to_string(),
                section: SectionInfo {
                    section_title: Some("Other".to_string()),
                    section_path: None,
                    heading_level: Some(3),
                },
            },
        ];
        let normalized = normalize_chunk_boundaries_sectioned(chunks);
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].text, "Valid text");
        assert_eq!(normalized[0].section.section_title.as_deref(), Some("Title"));
        assert_eq!(normalized[1].text, "word here");
        assert_eq!(normalized[1].section.section_title.as_deref(), Some("Other"));
    }

    #[test]
    fn normalize_chunk_boundaries_sectioned_filters_empty() {
        let chunks =
            vec![ChunkWithSection { text: "   ".to_string(), section: SectionInfo::default() }];
        let normalized = normalize_chunk_boundaries_sectioned(chunks);
        assert!(normalized.is_empty());
    }

    // ========================================================================
    // is_cjk_char
    // ========================================================================

    #[test]
    fn is_cjk_char_detects_cjk_unified_ideographs() {
        assert!(is_cjk_char('\u{4E00}'));
        assert!(is_cjk_char('\u{9FFF}'));
        assert!(is_cjk_char('中'));
    }

    #[test]
    fn is_cjk_char_detects_extension_a() {
        assert!(is_cjk_char('\u{3400}'));
        assert!(is_cjk_char('\u{4DBF}'));
    }

    #[test]
    fn is_cjk_char_detects_compatibility() {
        assert!(is_cjk_char('\u{F900}'));
        assert!(is_cjk_char('\u{FAFF}'));
    }

    #[test]
    fn is_cjk_char_detects_hiragana_and_katakana() {
        assert!(is_cjk_char('\u{3040}'));
        assert!(is_cjk_char('\u{309F}'));
        assert!(is_cjk_char('\u{30A0}'));
        assert!(is_cjk_char('\u{30FF}'));
        assert!(is_cjk_char('あ'));
        assert!(is_cjk_char('ア'));
    }

    #[test]
    fn is_cjk_char_rejects_non_cjk() {
        assert!(!is_cjk_char('A'));
        assert!(!is_cjk_char('z'));
        assert!(!is_cjk_char('é'));
        assert!(!is_cjk_char(' '));
        assert!(!is_cjk_char('\u{4DFF}'));
    }

    // ========================================================================
    // adjust_tokens_for_script
    // ========================================================================

    #[test]
    fn adjust_tokens_empty_text() {
        assert_eq!(adjust_tokens_for_script("", 600, 1.5), 600);
    }

    #[test]
    fn adjust_tokens_pure_latin() {
        assert_eq!(adjust_tokens_for_script("Hello world this is English text", 600, 1.5), 600);
    }

    #[test]
    fn adjust_tokens_pure_cjk() {
        let cjk = "机器学习深度学习自然语言处理计算机视觉";
        assert_eq!(adjust_tokens_for_script(cjk, 600, 1.5), 900);
    }

    #[test]
    fn adjust_tokens_mixed_below_threshold() {
        let mixed = "abc def ghi jkl 中文字";
        assert_eq!(adjust_tokens_for_script(mixed, 600, 1.5), 600);
    }

    #[test]
    fn adjust_tokens_multiplier_1_0_returns_unchanged() {
        assert_eq!(adjust_tokens_for_script("机器学习深度学习", 600, 1.0), 600);
    }

    #[test]
    fn adjust_tokens_multiplier_below_1_returns_unchanged() {
        assert_eq!(adjust_tokens_for_script("机器学习深度学习", 600, 0.5), 600);
    }

    // ========================================================================
    // ChunkingStrategy FromStr Aliases
    // ========================================================================

    #[test]
    fn strategy_from_str_aliases() {
        assert_eq!("page-aware".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::PageTokens);
        assert_eq!("pageaware".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::PageTokens);
        assert_eq!("characters".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Chars);
        assert_eq!("approx".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Chars);
        assert_eq!("paragraph".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Paragraphs);
        assert_eq!("md".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Markdown);
        assert_eq!("page".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Pages);
        assert_eq!(
            "recursive".parse::<ChunkingStrategy>().unwrap(),
            ChunkingStrategy::RecursiveChars
        );
    }

    #[test]
    fn strategy_from_str_invalid() {
        assert!("nonexistent".parse::<ChunkingStrategy>().is_err());
        assert!("".parse::<ChunkingStrategy>().is_err());
    }

    #[test]
    fn strategy_from_str_case_insensitive() {
        assert_eq!("TOKENS".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Tokens);
        assert_eq!("Markdown".parse::<ChunkingStrategy>().unwrap(), ChunkingStrategy::Markdown);
    }
}
