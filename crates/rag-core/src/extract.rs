//! Document format extraction primitives.
//!
//! The extractor interface is async from the start so OCR- or subprocess-based
//! extractors can be added later without a breaking trait change.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};

/// File formats currently recognized by the extraction layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    Pdf,
    Markdown,
    Text,
}

impl FileType {
    /// Infer a supported file type from a filename extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Normalized text extracted from a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub text: String,
    /// Native metadata from the source format (e.g. PDF document info).
    /// Non-PDF extractors return `None`. Date values (e.g. `creation_date`,
    /// `modification_date`) are stored raw in PDF date format
    /// (`D:YYYYMMDDHHmmSSOHH'mm'`), not ISO-8601.
    pub native_metadata: Option<HashMap<String, String>>,
}

/// Options that control extraction behavior beyond raw content.
///
/// Non-PDF extractors ignore these; `PdfExtractor` uses the `ocr` field
/// to decide whether and how to run Tesseract on scanned pages.
#[derive(Debug, Clone, Default)]
pub struct ExtractionOptions {
    pub ocr: Option<OcrOptions>,
}

/// Per-document OCR overrides, typically sourced from sidecar metadata.
#[derive(Debug, Clone, Default)]
pub struct OcrOptions {
    pub force: bool,
    pub language_hints: Vec<String>,
    pub timeout_secs: Option<u64>,
}

/// Compute a SHA-256 checksum over extracted text.
pub fn checksum(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Async-capable extractor interface for one or more file types.
pub trait FormatExtractor: Send + Sync {
    fn supported_types(&self) -> &'static [FileType];

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>>;
}

/// Registry that dispatches extraction requests by file type.
pub struct ExtractorRegistry {
    extractors: Vec<Box<dyn FormatExtractor>>,
    by_type: HashMap<FileType, usize>,
}

impl ExtractorRegistry {
    /// Build a registry from a concrete set of extractors.
    pub fn new(extractors: Vec<Box<dyn FormatExtractor>>) -> Result<Self> {
        let mut by_type = HashMap::new();

        for (index, extractor) in extractors.iter().enumerate() {
            for file_type in extractor.supported_types() {
                if by_type.insert(*file_type, index).is_some() {
                    bail!("duplicate extractor registration for file type {file_type:?}");
                }
            }
        }

        Ok(Self { extractors, by_type })
    }

    /// Construct the default registry, optionally including PDF support
    /// when `config.pdfium_library_path` is set.
    pub fn with_defaults(config: &crate::config::AppConfig) -> Result<Self> {
        let mut extractors: Vec<Box<dyn FormatExtractor>> =
            vec![Box::new(MarkdownExtractor), Box::new(TextExtractor)];

        if config.pdfium_library_path.is_some() {
            let pdf = PdfExtractor::new(config).context("configuring PDF extractor")?;
            extractors.push(Box::new(pdf));
        }

        Self::new(extractors)
    }

    /// Extract normalized text for the given file type.
    pub async fn extract(
        &self,
        file_type: FileType,
        content: &[u8],
        options: &ExtractionOptions,
    ) -> Result<ExtractionResult> {
        let Some(index) = self.by_type.get(&file_type).copied() else {
            if file_type == FileType::Pdf {
                bail!(
                    "no extractor registered for PDF files; \
                     set pdfium_library_path in config/app.toml \
                     or PDFIUM_LIBRARY_PATH env var to enable PDF support"
                );
            }
            bail!("no extractor registered for file type {file_type:?}");
        };

        self.extractors[index]
            .extract(content, options)
            .await
            .map_err(|err| err.context(format!("extracting content for file type {file_type:?}")))
    }
}

#[derive(Debug, Default)]
pub struct TextExtractor;

impl FormatExtractor for TextExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Text]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "plain text") })
    }
}

#[derive(Debug, Default)]
pub struct MarkdownExtractor;

impl FormatExtractor for MarkdownExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Markdown]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "markdown") })
    }
}

/// Extracts text from PDF files using the PDFium native library.
///
/// Stores the library path and creates a fresh PDFium binding on each
/// `extract()` call inside `spawn_blocking`. This avoids `Send + Sync`
/// issues with the C library handle.
#[derive(Debug)]
pub struct PdfExtractor {
    /// PDFium library path, stored as `String` because `bind_to_library()`
    /// requires `&str`. UTF-8 validity is checked once in `new()`.
    library_path: String,
    tessdata_dir: Option<std::path::PathBuf>,
    ocr_timeout_secs: u64,
    ocr_default_language: String,
}

impl PdfExtractor {
    /// Create a new `PdfExtractor` from application config.
    ///
    /// Consumes already-resolved config values -- `TESSDATA_PREFIX` env
    /// override happens in `AppConfig` loading, not here. Eagerly loads
    /// PDFium and validates tessdata (if configured) at startup.
    pub fn new(config: &crate::config::AppConfig) -> Result<Self> {
        let path_buf = config
            .pdfium_library_path
            .clone()
            .ok_or_else(|| anyhow!("pdfium_library_path is required for PdfExtractor"))?;

        let library_path = path_buf
            .to_str()
            .with_context(|| {
                format!("pdfium_library_path is not valid UTF-8: {}", path_buf.display())
            })?
            .to_owned();

        // Validate PDFium library is loadable at construction time.
        // Use bind_to_library() with the exact configured path — do NOT
        // use pdfium_platform_library_name_at_path(), which infers a
        // platform-default filename from a directory and would ignore
        // custom filenames, symlinks, or nonstandard install locations.
        drop(pdfium_render::prelude::Pdfium::new(
            pdfium_render::prelude::Pdfium::bind_to_library(&library_path).with_context(|| {
                format!(
                    "failed to load PDFium native library from {library_path}: \
                     verify the path points to a valid PDFium binary \
                     (.dylib on macOS, .so on Linux, .dll on Windows)",
                )
            })?,
        ));

        // Validate tessdata if configured.
        if let Some(ref dir) = config.tessdata_dir {
            let lang = &config.ocr_default_language;
            let traineddata = dir.join(format!("{lang}.traineddata"));
            if !traineddata.exists() {
                bail!(
                    "tessdata_dir {} does not contain {lang}.traineddata; \
                     install the language pack or adjust ocr_default_language",
                    dir.display()
                );
            }
        }

        Ok(Self {
            library_path,
            tessdata_dir: config.tessdata_dir.clone(),
            ocr_timeout_secs: config.ocr_timeout_secs,
            ocr_default_language: config.ocr_default_language.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Heading detection types
// ---------------------------------------------------------------------------

/// Collected text object from a PDF page, used for font analysis and line
/// coalescing. Positions are in PDF coordinates (origin bottom-left).
#[derive(Debug, Clone)]
struct ObjectSpan {
    text: String,
    font_size: f32,
    is_bold: bool,
    x_position: f32,
    y_position: f32,
    x_end: f32,
}

/// A line of text coalesced from one or more `ObjectSpan`s that share the
/// same approximate y-position.
#[derive(Debug, Clone)]
struct LineSpan {
    text: String,
    dominant_font_size: f32,
    is_bold: bool,
    y_position: f32,
    x_position: f32,
}

/// A heading accepted for marker insertion.
#[derive(Debug, Clone)]
struct AcceptedHeading {
    level: u8,
    text: String,
    y_position: f32,
    x_position: f32,
}

/// Font-size ratio thresholds for heading classification.
/// Hardcoded defaults; configurability deferred.
#[derive(Debug, Clone, Copy)]
struct HeadingThresholds {
    h1_ratio: f32,
    h2_ratio: f32,
    h3_ratio: f32,
    bold_min_ratio: f32,
    bold_as_h3: bool,
}

impl Default for HeadingThresholds {
    fn default() -> Self {
        Self { h1_ratio: 2.0, h2_ratio: 1.6, h3_ratio: 1.2, bold_min_ratio: 1.1, bold_as_h3: true }
    }
}

// ---------------------------------------------------------------------------
// Heading detection PDFium collectors
// ---------------------------------------------------------------------------

/// Collect text object spans from a PDF page for font analysis.
fn collect_object_spans(page: &pdfium_render::prelude::PdfPage<'_>) -> Vec<ObjectSpan> {
    use pdfium_render::prelude::{PdfPageObjectCommon, PdfPageObjectsCommon};

    let mut spans = Vec::new();

    for object in page.objects().iter() {
        let Some(text_obj) = object.as_text_object() else {
            continue;
        };
        let text = text_obj.text();
        if text.trim().is_empty() {
            continue;
        }
        let font_size = text_obj.unscaled_font_size().value;
        if !font_size.is_finite() || font_size <= 0.0 {
            continue;
        }

        let is_bold = is_bold_weight(text_obj.font().weight().ok());

        let (x_position, y_position, x_end) = match object.bounds() {
            Ok(bounds) => (bounds.left().value, bounds.bottom().value, bounds.right().value),
            Err(_) => continue,
        };

        spans.push(ObjectSpan { text, font_size, is_bold, x_position, y_position, x_end });
    }

    spans
}

/// Check if a font weight indicates bold (>= 700).
fn is_bold_weight(weight: Option<pdfium_render::prelude::PdfFontWeight>) -> bool {
    use pdfium_render::prelude::PdfFontWeight;
    match weight {
        Some(PdfFontWeight::Weight700Bold)
        | Some(PdfFontWeight::Weight800)
        | Some(PdfFontWeight::Weight900) => true,
        Some(PdfFontWeight::Custom(value)) => value >= 700,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Heading detection helpers
// ---------------------------------------------------------------------------

/// Normalize text by trimming and collapsing internal whitespace to single
/// spaces. Returns the normalized string and a byte-offset map where
/// `map[normalized_byte_pos] = original_byte_pos`.
fn normalize_with_offset_map(text: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(text.len());
    let mut offset_map: Vec<usize> = Vec::with_capacity(text.len());
    let mut in_whitespace = true; // start true to trim leading

    for (byte_idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if !in_whitespace && !normalized.is_empty() {
                // Emit a single space for the first whitespace char in a run.
                normalized.push(' ');
                offset_map.push(byte_idx);
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            let start = normalized.len();
            normalized.push(ch);
            // Map each byte of this char to its original byte offset.
            for i in 0..ch.len_utf8() {
                if start + i >= offset_map.len() {
                    offset_map.push(byte_idx + i);
                }
            }
        }
    }

    // Trim trailing space.
    if normalized.ends_with(' ') {
        normalized.pop();
        offset_map.pop();
    }

    (normalized, offset_map)
}

/// Compute the body (most common) font size from object spans using a
/// histogram mode weighted by alphabetic character count.
///
/// Returns `None` if no usable samples exist or the winning bucket covers
/// less than 20% of total alphabetic characters (stability gate).
fn compute_body_font_size(spans: &[ObjectSpan]) -> Option<f32> {
    if spans.is_empty() {
        return None;
    }

    let mut buckets: HashMap<i32, usize> = HashMap::new();
    let mut total_alpha: usize = 0;

    for span in spans {
        if !span.font_size.is_finite() || span.font_size <= 0.0 {
            continue;
        }
        let alpha_count = span.text.chars().filter(|c| c.is_alphabetic()).count();
        if alpha_count == 0 {
            continue;
        }
        let bucket = (span.font_size * 10.0).round() as i32;
        *buckets.entry(bucket).or_insert(0) += alpha_count;
        total_alpha += alpha_count;
    }

    if total_alpha == 0 {
        return None;
    }

    // Select bucket with highest weight; tie-break to smaller font size.
    let (best_bucket, best_weight) =
        buckets.into_iter().max_by(|(bucket_a, weight_a), (bucket_b, weight_b)| {
            weight_a.cmp(weight_b).then_with(|| bucket_b.cmp(bucket_a))
        })?;

    // Stability gate: winning bucket must cover >= 20% of total alpha chars.
    if (best_weight as f64) < (total_alpha as f64 * 0.2) {
        return None;
    }

    Some(best_bucket as f32 / 10.0)
}

/// Coalesce `ObjectSpan`s into `LineSpan`s by y-position clustering.
///
/// Two objects share a line when their y-positions differ by less than
/// `min(font_a, font_b) * 0.5`. Within a line, objects are sorted by
/// x-position and concatenated with appropriate spacing.
fn coalesce_into_lines(mut spans: Vec<ObjectSpan>) -> Vec<LineSpan> {
    if spans.is_empty() {
        return Vec::new();
    }

    // Sort by y descending (top-to-bottom in PDF coords), then x ascending.
    spans.sort_by(|a, b| {
        b.y_position.partial_cmp(&a.y_position).unwrap_or(std::cmp::Ordering::Equal).then_with(
            || a.x_position.partial_cmp(&b.x_position).unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let mut lines: Vec<Vec<ObjectSpan>> = Vec::new();

    for span in spans {
        let match_idx = lines.iter().position(|line| {
            let representative = &line[0];
            let tolerance = span.font_size.min(representative.font_size) * 0.5;
            (span.y_position - representative.y_position).abs() < tolerance
        });
        match match_idx {
            Some(idx) => lines[idx].push(span),
            None => lines.push(vec![span]),
        }
    }

    lines
        .into_iter()
        .map(|mut group| {
            group.sort_by(|a, b| {
                a.x_position.partial_cmp(&b.x_position).unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut text = String::new();
            let mut weighted_size_sum: f64 = 0.0;
            let mut total_chars: usize = 0;
            let mut bold_alpha: usize = 0;
            let mut total_alpha: usize = 0;
            let mut prev_x_end: Option<f32> = None;
            let mut prev_ends_hyphen = false;

            let first_y = group[0].y_position;
            let first_x = group[0].x_position;

            for span in &group {
                let alpha_count = span.text.chars().filter(|c| c.is_alphabetic()).count();
                let char_count = span.text.chars().count();
                weighted_size_sum += span.font_size as f64 * char_count as f64;
                total_chars += char_count;
                total_alpha += alpha_count;
                if span.is_bold {
                    bold_alpha += alpha_count;
                }

                if !text.is_empty() {
                    let needs_space = !prev_ends_hyphen
                        && prev_x_end.map(|end| span.x_position > end).unwrap_or(true);
                    if needs_space {
                        text.push(' ');
                    }
                }
                text.push_str(&span.text);
                prev_x_end = Some(span.x_end);
                prev_ends_hyphen = span.text.ends_with('-');
            }

            let dominant_font_size =
                if total_chars > 0 { (weighted_size_sum / total_chars as f64) as f32 } else { 0.0 };

            let is_bold = total_alpha > 0 && bold_alpha * 2 > total_alpha;

            LineSpan { text, dominant_font_size, is_bold, y_position: first_y, x_position: first_x }
        })
        .collect()
}

/// Check if a line of text is a plausible heading candidate.
///
/// Filters: 3-180 chars, >= 3 alphabetic chars, >= 50% alphabetic density,
/// <= 20 words, < 3 sentence-ending punctuation marks.
fn is_heading_candidate(text: &str) -> bool {
    let char_count = text.chars().count();
    if char_count < 3 || char_count > 180 {
        return false;
    }

    let alpha_count = text.chars().filter(|c| c.is_alphabetic()).count();
    if alpha_count < 3 {
        return false;
    }

    if alpha_count * 2 < char_count {
        return false;
    }

    if text.split_whitespace().count() > 20 {
        return false;
    }

    let sentence_ends = text.chars().filter(|&c| c == '.' || c == '?' || c == '!').count();
    if sentence_ends >= 3 {
        return false;
    }

    true
}

/// Classify line spans as headings based on font-size ratio to body size.
///
/// Returns accepted headings in the order they appear in `lines`.
fn classify_line_headings(
    lines: &[LineSpan],
    body_size: f32,
    thresholds: &HeadingThresholds,
) -> Vec<AcceptedHeading> {
    let mut headings = Vec::new();

    for line in lines {
        if !is_heading_candidate(&line.text) {
            continue;
        }

        // Use f64 arithmetic to reduce floating-point precision loss when
        // comparing font-size ratios. A tiny epsilon absorbs rounding from
        // f32 storage (e.g. 14.4f32 / 12.0f32 is slightly below 1.2f32).
        const RATIO_EPS: f64 = 1e-6;
        let ratio = line.dominant_font_size as f64 / body_size as f64;

        let level = if ratio >= thresholds.h1_ratio as f64 - RATIO_EPS {
            Some(1u8)
        } else if ratio >= thresholds.h2_ratio as f64 - RATIO_EPS {
            Some(2)
        } else if ratio >= thresholds.h3_ratio as f64 - RATIO_EPS {
            Some(3)
        } else if thresholds.bold_as_h3
            && line.is_bold
            && ratio >= thresholds.bold_min_ratio as f64 - RATIO_EPS
        {
            let has_sentence_end = line.text.chars().any(|c| c == '.' || c == '?' || c == '!');
            if has_sentence_end { None } else { Some(3) }
        } else {
            None
        };

        if let Some(level) = level {
            headings.push(AcceptedHeading {
                level,
                text: line.text.clone(),
                y_position: line.y_position,
                x_position: line.x_position,
            });
        }
    }

    headings
}

/// Insert markdown heading markers into page text at positions found by
/// monotonic left-to-right matching. Returns the original text unchanged
/// if no headings could be matched.
// tracing::debug! internally uses .expect()
#[allow(clippy::disallowed_methods)]
fn insert_heading_markers(page_text: &str, headings: &[AcceptedHeading]) -> String {
    if headings.is_empty() {
        return page_text.to_string();
    }

    let (normalized_page, offset_map) = normalize_with_offset_map(page_text);

    struct MatchedHeading {
        original_byte_start: usize,
        original_byte_end: usize,
        level: u8,
        text: String,
    }

    let mut matched: Vec<MatchedHeading> = Vec::new();
    let mut search_start: usize = 0;

    for heading in headings {
        let (normalized_heading, _) = normalize_with_offset_map(&heading.text);
        if normalized_heading.is_empty() {
            continue;
        }

        let Some(pos) = normalized_page[search_start..].find(&normalized_heading) else {
            tracing::debug!(
                heading_text = %heading.text,
                "heading candidate could not be matched in page text, skipping"
            );
            continue;
        };

        let norm_start = search_start + pos;
        let norm_end = norm_start + normalized_heading.len();

        let orig_start = offset_map[norm_start];
        let orig_end =
            if norm_end < offset_map.len() { offset_map[norm_end] } else { page_text.len() };

        matched.push(MatchedHeading {
            original_byte_start: orig_start,
            original_byte_end: orig_end,
            level: heading.level,
            text: heading.text.clone(),
        });

        search_start = norm_end;
    }

    if matched.is_empty() {
        return page_text.to_string();
    }

    // Insert markers in reverse byte-offset order to preserve positions.
    let mut result = page_text.to_string();
    for m in matched.iter().rev() {
        let prefix = match m.level {
            1 => "# ",
            2 => "## ",
            _ => "### ",
        };
        let replacement = format!("\n{prefix}{}\n", m.text);
        result.replace_range(m.original_byte_start..m.original_byte_end, &replacement);
    }

    result
}

/// Orchestrator: detect headings on a native-text PDF page and insert
/// inline markdown markers. Returns the original text on any failure or
/// when no headings are detected.
// tracing macros internally use .expect()
#[allow(clippy::disallowed_methods)]
fn detect_and_insert_headings(
    page: &pdfium_render::prelude::PdfPage<'_>,
    native_text: &str,
    page_index: usize,
) -> Result<String> {
    let spans = collect_object_spans(page);

    tracing::debug!(page_index, page_objects_count = spans.len(), "collected text object spans");

    if spans.is_empty() {
        return Ok(native_text.to_string());
    }

    let body_size = match compute_body_font_size(&spans) {
        Some(size) => {
            tracing::debug!(page_index, body_font_size = size, "computed body font size");
            size
        }
        None => {
            tracing::debug!(page_index, "no stable body font size, skipping heading detection");
            return Ok(native_text.to_string());
        }
    };

    let lines = coalesce_into_lines(spans);
    let thresholds = HeadingThresholds::default();
    let headings = classify_line_headings(&lines, body_size, &thresholds);

    if headings.is_empty() {
        tracing::debug!(page_index, "no headings classified");
        return Ok(native_text.to_string());
    }

    let result = insert_heading_markers(native_text, &headings);

    let accepted = headings.len();
    let markers_found = result.matches("\n# ").count()
        + result.matches("\n## ").count()
        + result.matches("\n### ").count();

    tracing::debug!(
        page_index,
        headings_accepted = accepted,
        candidates_skipped = accepted.saturating_sub(markers_found),
        "heading detection complete"
    );

    Ok(result)
}

impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    // tracing::warn! internally uses .expect()
    #[allow(clippy::disallowed_methods)]
    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        let owned_bytes = content.to_vec();
        let lib_path = self.library_path.clone();
        let tessdata_dir = self.tessdata_dir.clone();
        let default_language = self.ocr_default_language.clone();
        let default_timeout = self.ocr_timeout_secs;
        let ocr_options = options.ocr.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let pdfium = pdfium_render::prelude::Pdfium::new(
                    pdfium_render::prelude::Pdfium::bind_to_library(&lib_path)
                        .with_context(|| format!("binding PDFium library from {lib_path}"))?,
                );

                let doc = pdfium
                    .load_pdf_from_byte_vec(owned_bytes, None)
                    .map_err(|e| anyhow!("loading PDF document: {e}"))?;

                use pdfium_render::prelude::PdfDocumentMetadataTagType;

                let native_metadata: HashMap<String, String> = doc
                    .metadata()
                    .iter()
                    .filter_map(|tag| {
                        let key = match tag.tag_type() {
                            PdfDocumentMetadataTagType::Title => "title",
                            PdfDocumentMetadataTagType::Author => "author",
                            PdfDocumentMetadataTagType::Subject => "subject",
                            PdfDocumentMetadataTagType::Keywords => "keywords",
                            PdfDocumentMetadataTagType::Creator => "creator",
                            PdfDocumentMetadataTagType::Producer => "producer",
                            PdfDocumentMetadataTagType::CreationDate => "creation_date",
                            PdfDocumentMetadataTagType::ModificationDate => "modification_date",
                        };
                        let value = tag.value();
                        if value.is_empty() {
                            None
                        } else {
                            Some((key.to_string(), value.to_string()))
                        }
                    })
                    .collect();
                let native_metadata =
                    if native_metadata.is_empty() { None } else { Some(native_metadata) };

                // Resolve OCR settings once before page loop.
                let settings =
                    resolve_ocr_settings(ocr_options.as_ref(), &default_language, default_timeout);
                let ocr_available = tessdata_dir.is_some();

                // Fail early if forced but unavailable.
                if settings.force && !ocr_available {
                    bail!(
                        "OCR forced but Tesseract not configured \
                         (tessdata_dir not set)"
                    );
                }

                let mut pages = Vec::new();
                let mut ocr_skipped_pages: u32 = 0;

                for page in doc.pages().iter() {
                    let native_text = page
                        .text()
                        .map_err(|e| anyhow!("extracting text from PDF page: {e}"))?
                        .all();

                    match page_ocr_decision(settings.force, &native_text, ocr_available) {
                        PageOcrDecision::UseNativeText => {
                            let page_idx = pages.len();
                            let text = detect_and_insert_headings(&page, &native_text, page_idx)
                                .unwrap_or(native_text);
                            pages.push(text);
                        }
                        PageOcrDecision::SkipUnavailable => {
                            ocr_skipped_pages += 1;
                            pages.push(native_text);
                        }
                        PageOcrDecision::PerformOcr => {
                            let tessdata = tessdata_dir
                                .as_ref()
                                .ok_or_else(|| anyhow!("tessdata_dir is None"))?;

                            let png_bytes = render_page_to_png(&page)?;

                            let ocr_text = run_tesseract(
                                &png_bytes,
                                &settings.language,
                                settings.timeout,
                                tessdata,
                            )?;

                            pages.push(ocr_text);
                        }
                    }
                }

                if ocr_skipped_pages > 0 {
                    if ocr_skipped_pages as usize == pages.len() {
                        bail!(
                            "all {ocr_skipped_pages} page(s) need OCR but \
                             Tesseract is not configured (tessdata_dir not set)"
                        );
                    }
                    tracing::warn!(
                        skipped_pages = ocr_skipped_pages,
                        "OCR fallback skipped for \
                         {ocr_skipped_pages} page(s) because \
                         Tesseract is not configured"
                    );
                }

                Ok(ExtractionResult { text: pages.join("\u{000C}"), native_metadata })
            })
            .await
            .context("PDF extraction task panicked")?
        })
    }
}

fn extract_utf8_passthrough(content: &[u8], format_name: &str) -> Result<ExtractionResult> {
    let text = std::str::from_utf8(content)
        .with_context(|| format!("decoding {format_name} content as UTF-8"))?
        .to_owned();

    Ok(ExtractionResult { text, native_metadata: None })
}

// ---------------------------------------------------------------------------
// OCR decision helpers
// ---------------------------------------------------------------------------

/// Outcome of the per-page OCR decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageOcrDecision {
    /// Page has native text and force is off -- use PDFium output.
    UseNativeText,
    /// Page needs OCR and Tesseract is available -- render + OCR.
    PerformOcr,
    /// Page needs OCR but Tesseract is unavailable -- increment
    /// skip counter and use native (empty) text.
    SkipUnavailable,
}

/// Determine the OCR outcome for a single page.
///
/// Note: the `force + !ocr_available` hard-error case is handled
/// before the page loop (early bail). This function is only called
/// when that combination is impossible, so `SkipUnavailable` only
/// arises for automatic (non-forced) OCR triggers.
fn page_ocr_decision(force: bool, native_text: &str, ocr_available: bool) -> PageOcrDecision {
    let needs_ocr = force || native_text.trim().is_empty();
    if !needs_ocr {
        PageOcrDecision::UseNativeText
    } else if ocr_available {
        PageOcrDecision::PerformOcr
    } else {
        PageOcrDecision::SkipUnavailable
    }
}

/// Pre-loop OCR settings resolved from sidecar options + config defaults.
struct ResolvedOcrSettings {
    force: bool,
    language: String,
    timeout: std::time::Duration,
}

/// Resolve effective OCR settings once before the page loop.
fn resolve_ocr_settings(
    options: Option<&OcrOptions>,
    default_language: &str,
    default_timeout_secs: u64,
) -> ResolvedOcrSettings {
    let opts = options.cloned().unwrap_or_default();

    let language = if opts.language_hints.is_empty() {
        default_language.to_string()
    } else {
        opts.language_hints.join("+")
    };

    let force = opts.force;
    let timeout_override = opts.timeout_secs;

    let raw_secs = timeout_override.unwrap_or(default_timeout_secs);
    // A zero timeout would cause immediate kill — clamp to 1s minimum.
    let timeout = std::time::Duration::from_secs(raw_secs.max(1));

    ResolvedOcrSettings { force, language, timeout }
}

/// DPI used for rendering PDF pages to bitmaps before OCR.
const OCR_RENDER_DPI: f32 = 300.0;

/// Render a PDF page to in-memory PNG bytes for OCR processing.
///
/// Uses PDFium's bitmap rendering at 300 DPI, then encodes to PNG
/// via the `image` crate.
///
/// Must be called from a blocking context (inside `spawn_blocking`).
fn render_page_to_png(page: &pdfium_render::prelude::PdfPage<'_>) -> Result<Vec<u8>> {
    let scale = OCR_RENDER_DPI / 72.0;
    let width_f = page.width().value * scale;
    let height_f = page.height().value * scale;

    if width_f < 1.0 || height_f < 1.0 || width_f >= i32::MAX as f32 || height_f >= i32::MAX as f32
    {
        bail!(
            "PDF page dimensions out of range for OCR rendering \
             ({width_f:.0} x {height_f:.0} px at {OCR_RENDER_DPI} DPI)"
        );
    }

    let width = width_f as i32;
    let height = height_f as i32;

    let config = pdfium_render::prelude::PdfRenderConfig::new()
        .set_target_width(width)
        .set_maximum_height(height);

    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("rendering PDF page to bitmap: {e}"))?;

    let image = bitmap.as_image();
    let mut cursor = std::io::Cursor::new(Vec::new());
    image.write_to(&mut cursor, image::ImageFormat::Png).context("encoding page bitmap as PNG")?;

    Ok(cursor.into_inner())
}

/// Run Tesseract OCR on PNG image bytes via stdin and return text.
///
/// Pipes `png_bytes` to `tesseract stdin stdout`, reads extracted text
/// from stdout. Uses deadline-based timeout with `try_wait()` polling
/// and explicit `child.kill()` -- `tokio::time::timeout` does NOT kill
/// child processes.
///
/// Must be called from a blocking context (inside `spawn_blocking`).
fn run_tesseract(
    png_bytes: &[u8],
    language: &str,
    timeout: std::time::Duration,
    tessdata_dir: &std::path::Path,
) -> Result<String> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("tesseract");
    cmd.arg("stdin").arg("stdout").arg("-l").arg(language).arg("--tessdata-dir").arg(tessdata_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning tesseract subprocess")?;

    // Take all three pipes before spawning threads.
    let mut stdin_pipe =
        child.stdin.take().ok_or_else(|| anyhow!("missing tesseract stdin pipe"))?;
    let mut stdout_pipe =
        child.stdout.take().ok_or_else(|| anyhow!("missing tesseract stdout pipe"))?;
    let mut stderr_pipe =
        child.stderr.take().ok_or_else(|| anyhow!("missing tesseract stderr pipe"))?;

    // Write PNG to stdin on a separate thread to avoid deadlock.
    let owned_bytes = png_bytes.to_vec();
    let stdin_thread = std::thread::spawn(move || {
        let result = std::io::Write::write_all(&mut stdin_pipe, &owned_bytes);
        drop(stdin_pipe); // Close stdin to signal EOF.
        result
    });

    // Read stdout/stderr on separate threads.
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stdout_pipe, &mut buf).map(|_| buf)
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stderr_pipe, &mut buf).map(|_| buf)
    });

    // Poll with try_wait() -- tokio::time::timeout does NOT kill
    // child processes.
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().context("waiting for tesseract")? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                child.kill().ok();
                child.wait().ok();
                let _ = stdin_thread.join();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!("tesseract timed out (exceeded {} ms)", timeout.as_millis());
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };

    // Collect stdin result WITHOUT propagating yet — if the child
    // exits early with an error, the stdin writer gets BrokenPipe
    // which would hide the real diagnostic from stderr.
    let stdin_result =
        stdin_thread.join().map_err(|_| anyhow!("tesseract stdin writer panicked"))?;

    let stdout_bytes = stdout_thread
        .join()
        .map_err(|_| anyhow!("tesseract stdout reader panicked"))?
        .context("reading tesseract stdout")?;
    let stderr_bytes = stderr_thread
        .join()
        .map_err(|_| anyhow!("tesseract stderr reader panicked"))?
        .context("reading tesseract stderr")?;

    // Check exit status first — surfaces the stderr diagnostic
    // instead of a misleading BrokenPipe from stdin.
    if !status.success() {
        bail!("tesseract failed: {}", String::from_utf8_lossy(&stderr_bytes));
    }

    // Only propagate stdin errors when Tesseract itself succeeded.
    stdin_result.context("writing PNG to tesseract stdin")?;

    let text = String::from_utf8(stdout_bytes).context("tesseract output is not valid UTF-8")?;
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a test registry with text + markdown extractors (no PDF — no native lib in CI).
    #[allow(clippy::disallowed_methods)] // test helper
    fn test_registry() -> ExtractorRegistry {
        ExtractorRegistry::new(vec![Box::new(MarkdownExtractor), Box::new(TextExtractor)])
            .expect("test registry should build")
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn text_registry_extracts_utf8_content() {
        let registry = test_registry();

        let result = registry
            .extract(FileType::Text, b"hello world", &ExtractionOptions::default())
            .await
            .expect("text extraction should succeed");

        assert_eq!(result.text, "hello world");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn markdown_registry_extracts_utf8_content() {
        let registry = test_registry();

        let result = registry
            .extract(FileType::Markdown, b"# Title\n\nBody", &ExtractionOptions::default())
            .await
            .expect("markdown extraction should succeed");

        assert_eq!(result.text, "# Title\n\nBody");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn registry_rejects_duplicate_file_type_registration() {
        struct DuplicateTextExtractor;

        impl FormatExtractor for DuplicateTextExtractor {
            fn supported_types(&self) -> &'static [FileType] {
                &[FileType::Text]
            }

            fn extract<'a>(
                &'a self,
                content: &'a [u8],
                _options: &'a ExtractionOptions,
            ) -> BoxFuture<'a, Result<ExtractionResult>> {
                Box::pin(async move { extract_utf8_passthrough(content, "plain text") })
            }
        }

        let err = match ExtractorRegistry::new(vec![
            Box::new(TextExtractor),
            Box::new(DuplicateTextExtractor),
        ]) {
            Ok(_) => panic!("duplicate file type registration should fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("duplicate extractor registration"),
            "expected duplicate registration error, got {err}"
        );
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn registry_without_pdf_returns_config_hint() {
        // No PDF extractor — simulates the unconfigured case.
        let registry = test_registry();

        let err = registry
            .extract(FileType::Pdf, b"%PDF-1.7", &ExtractionOptions::default())
            .await
            .expect_err("PDF extraction should fail when not configured");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("pdfium_library_path"),
            "error should name the missing config option, got: {msg}"
        );
        assert!(msg.contains("PDFIUM_LIBRARY_PATH"), "error should name the env var, got: {msg}");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn invalid_utf8_errors_keep_decode_cause_visible() {
        let registry = test_registry();

        let err = registry
            .extract(FileType::Text, &[0xff, 0xfe, 0xfd], &ExtractionOptions::default())
            .await
            .expect_err("invalid UTF-8 should fail");

        assert!(
            format!("{err:#}").contains("decoding plain text content as UTF-8"),
            "expected UTF-8 decode cause in error chain, got {err:#}"
        );
        assert!(
            err.chain().any(|cause| cause.is::<std::str::Utf8Error>()),
            "expected underlying Utf8Error to remain in the chain, got {err:#}"
        );
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn non_pdf_extractors_return_no_native_metadata() {
        let registry = test_registry();

        let result = registry
            .extract(FileType::Text, b"hello", &ExtractionOptions::default())
            .await
            .expect("text extraction should succeed");

        assert!(
            result.native_metadata.is_none(),
            "non-PDF extractors should return native_metadata: None"
        );
    }

    #[test]
    fn extraction_options_default_has_no_ocr() {
        let opts = ExtractionOptions::default();
        assert!(opts.ocr.is_none());
    }

    #[test]
    fn checksum_is_stable_for_extracted_text() {
        assert_eq!(
            checksum("hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // --- page_ocr_decision tests ---

    #[test]
    fn page_ocr_decision_native_text_no_force() {
        assert_eq!(page_ocr_decision(false, "content", true), PageOcrDecision::UseNativeText,);
        assert_eq!(page_ocr_decision(false, "content", false), PageOcrDecision::UseNativeText,);
    }

    #[test]
    fn page_ocr_decision_empty_text_ocr_available() {
        assert_eq!(page_ocr_decision(false, "", true), PageOcrDecision::PerformOcr,);
        assert_eq!(page_ocr_decision(false, "   ", true), PageOcrDecision::PerformOcr,);
        assert_eq!(page_ocr_decision(false, "\n\t ", true), PageOcrDecision::PerformOcr,);
    }

    #[test]
    fn page_ocr_decision_empty_text_ocr_unavailable() {
        assert_eq!(page_ocr_decision(false, "", false), PageOcrDecision::SkipUnavailable,);
    }

    #[test]
    fn page_ocr_decision_force_overrides_native_text() {
        assert_eq!(page_ocr_decision(true, "has content", true), PageOcrDecision::PerformOcr,);
    }

    // --- resolve_ocr_settings tests ---

    #[test]
    fn resolve_ocr_settings_uses_defaults() {
        let resolved = resolve_ocr_settings(None, "eng", 30);
        assert!(!resolved.force);
        assert_eq!(resolved.language, "eng");
        assert_eq!(resolved.timeout.as_secs(), 30);
    }

    #[test]
    fn resolve_ocr_settings_joins_language_hints() {
        let opts = OcrOptions {
            force: true,
            language_hints: vec!["eng".into(), "deu".into()],
            timeout_secs: Some(60),
        };
        let resolved = resolve_ocr_settings(Some(&opts), "fra", 30);
        assert!(resolved.force);
        assert_eq!(resolved.language, "eng+deu");
        assert_eq!(resolved.timeout.as_secs(), 60);
    }

    #[test]
    fn resolve_ocr_settings_empty_hints_uses_default_language() {
        let opts = OcrOptions { force: false, language_hints: vec![], timeout_secs: None };
        let resolved = resolve_ocr_settings(Some(&opts), "fra", 45);
        assert_eq!(resolved.language, "fra");
        assert_eq!(resolved.timeout.as_secs(), 45);
    }

    #[test]
    fn resolve_ocr_settings_clamps_zero_timeout() {
        let opts = OcrOptions { force: false, language_hints: vec![], timeout_secs: Some(0) };
        let resolved = resolve_ocr_settings(Some(&opts), "eng", 30);
        assert_eq!(resolved.timeout.as_secs(), 1, "zero timeout should be clamped to 1s");
    }

    #[test]
    fn heading_thresholds_default_values() {
        let t = HeadingThresholds::default();
        assert!((t.h1_ratio - 2.0).abs() < f32::EPSILON);
        assert!((t.h2_ratio - 1.6).abs() < f32::EPSILON);
        assert!((t.h3_ratio - 1.2).abs() < f32::EPSILON);
        assert!((t.bold_min_ratio - 1.1).abs() < f32::EPSILON);
        assert!(t.bold_as_h3);
    }

    // --- OCR integration test helpers ---

    /// Find a usable tessdata directory from common system locations.
    fn find_tessdata_dir() -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("TESSDATA_PREFIX") {
            let path = std::path::PathBuf::from(dir);
            if path.exists() {
                return Some(path);
            }
        }
        for dir in &[
            "/opt/homebrew/share/tessdata",
            "/usr/local/share/tessdata",
            "/usr/share/tesseract-ocr/5/tessdata",
            "/usr/share/tesseract-ocr/4.00/tessdata",
            "/usr/share/tessdata",
        ] {
            let path = std::path::PathBuf::from(dir);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Check whether tesseract is installed and tessdata is available.
    fn tesseract_available() -> Option<std::path::PathBuf> {
        if std::process::Command::new("tesseract").arg("--version").output().is_err() {
            return None;
        }
        find_tessdata_dir()
    }

    /// Build an AppConfig suitable for PdfExtractor integration tests.
    fn test_pdf_config(
        tessdata_dir: Option<std::path::PathBuf>,
    ) -> Option<crate::config::AppConfig> {
        let pdfium_path = std::env::var("PDFIUM_LIBRARY_PATH")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())?;

        let mut config = crate::config::AppConfig::from_env().ok()?;
        config.pdfium_library_path = Some(pdfium_path);
        config.tessdata_dir = tessdata_dir;
        config.ocr_timeout_secs = 30;
        config.ocr_default_language = "eng".to_string();
        Some(config)
    }

    #[test]
    #[ignore] // requires tesseract installed
    #[allow(clippy::disallowed_methods)]
    fn run_tesseract_extracts_text_from_png_bytes() {
        let tessdata = match tesseract_available() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: tesseract or tessdata not found");
                return;
            }
        };

        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.png");
        if !fixture.exists() {
            eprintln!("skipping: fixture not found at {}", fixture.display());
            return;
        }

        let png_bytes = std::fs::read(&fixture).expect("reading PNG fixture");

        let text = run_tesseract(&png_bytes, "eng", std::time::Duration::from_secs(30), &tessdata)
            .expect("tesseract should succeed on fixture");

        assert!(
            text.to_uppercase().contains("HELLO"),
            "OCR output should contain 'HELLO', got: {text:?}"
        );
    }

    #[tokio::test]
    #[ignore] // requires PDFium + Tesseract installed
    #[allow(clippy::disallowed_methods)]
    async fn pdf_extractor_ocr_fallback_on_scanned_page() {
        let tessdata = match tesseract_available() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: tesseract or tessdata not found");
                return;
            }
        };

        let config = match test_pdf_config(Some(tessdata)) {
            Some(c) => c,
            None => {
                eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
                return;
            }
        };

        let extractor = PdfExtractor::new(&config).expect("PdfExtractor should construct");

        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.pdf");
        if !fixture.exists() {
            eprintln!("skipping: PDF fixture not found at {}", fixture.display());
            return;
        }

        let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

        let result = extractor
            .extract(&pdf_bytes, &ExtractionOptions::default())
            .await
            .expect("extraction should succeed with OCR fallback");

        assert!(
            result.text.to_uppercase().contains("HELLO"),
            "OCR fallback output should contain 'HELLO', got: {:?}",
            result.text
        );
    }

    #[tokio::test]
    #[ignore] // requires PDFium installed (no Tesseract needed)
    #[allow(clippy::disallowed_methods)]
    async fn pdf_extractor_forced_ocr_without_tessdata_errors() {
        let config = match test_pdf_config(None) {
            Some(c) => c,
            None => {
                eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
                return;
            }
        };

        let extractor =
            PdfExtractor::new(&config).expect("PdfExtractor should construct without tessdata");

        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.pdf");
        if !fixture.exists() {
            eprintln!("skipping: PDF fixture not found at {}", fixture.display());
            return;
        }

        let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

        let options = ExtractionOptions {
            ocr: Some(OcrOptions { force: true, language_hints: vec![], timeout_secs: None }),
        };

        let err = extractor
            .extract(&pdf_bytes, &options)
            .await
            .expect_err("forced OCR without tessdata should fail");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("not configured"),
            "error should mention OCR not configured, got: {msg}"
        );
    }

    // --- normalize_with_offset_map tests ---

    #[test]
    fn normalize_with_offset_map_collapses_whitespace() {
        let (normalized, map) = normalize_with_offset_map("  hello   world  ");
        assert_eq!(normalized, "hello world");
        // 'h' at normalized byte 0 maps to original byte 2
        assert_eq!(map[0], 2);
        // 'w' at normalized byte 6 maps to original byte 10
        assert_eq!(map[6], 10);
    }

    #[test]
    fn normalize_with_offset_map_tabs_and_newlines() {
        let (normalized, map) = normalize_with_offset_map("foo\t\n  bar");
        assert_eq!(normalized, "foo bar");
        // 'b' at normalized byte 4 maps to original byte 7
        assert_eq!(map[4], 7);
    }

    #[test]
    fn normalize_with_offset_map_empty_input() {
        let (normalized, map) = normalize_with_offset_map("");
        assert_eq!(normalized, "");
        assert!(map.is_empty());
    }

    #[test]
    fn normalize_with_offset_map_no_whitespace() {
        let (normalized, map) = normalize_with_offset_map("abc");
        assert_eq!(normalized, "abc");
        assert_eq!(map.len(), 3);
        assert_eq!(map[0], 0);
        assert_eq!(map[1], 1);
        assert_eq!(map[2], 2);
    }

    #[test]
    fn normalize_with_offset_map_preserves_case_and_punctuation() {
        let (normalized, _) = normalize_with_offset_map("  Hello, World!  ");
        assert_eq!(normalized, "Hello, World!");
    }

    #[test]
    fn body_font_size_picks_mode() {
        let spans = vec![
            ObjectSpan {
                text: "a".repeat(100),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 0.0,
                x_end: 100.0,
            },
            ObjectSpan {
                text: "b".repeat(20),
                font_size: 24.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 50.0,
                x_end: 100.0,
            },
        ];
        let result = compute_body_font_size(&spans);
        assert_eq!(result, Some(12.0));
    }

    #[test]
    fn body_font_size_tie_breaks_to_smaller() {
        let spans = vec![
            ObjectSpan {
                text: "a".repeat(50),
                font_size: 14.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 0.0,
                x_end: 100.0,
            },
            ObjectSpan {
                text: "b".repeat(50),
                font_size: 16.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 50.0,
                x_end: 100.0,
            },
        ];
        let result = compute_body_font_size(&spans);
        assert_eq!(result, Some(14.0));
    }

    #[test]
    fn body_font_size_stability_gate_rejects_low_coverage() {
        let mut spans = vec![ObjectSpan {
            text: "a".repeat(10),
            font_size: 12.0,
            is_bold: false,
            x_position: 0.0,
            y_position: 0.0,
            x_end: 100.0,
        }];
        for i in 1..10 {
            spans.push(ObjectSpan {
                text: "b".repeat(10),
                font_size: 12.0 + i as f32 * 2.0,
                is_bold: false,
                x_position: 0.0,
                y_position: i as f32 * 50.0,
                x_end: 100.0,
            });
        }
        let result = compute_body_font_size(&spans);
        assert_eq!(result, None, "no bucket reaches 20% coverage");
    }

    #[test]
    fn body_font_size_empty_spans() {
        let result = compute_body_font_size(&[]);
        assert_eq!(result, None);
    }

    #[test]
    fn body_font_size_ignores_non_alphabetic() {
        let spans = vec![
            ObjectSpan {
                text: "12345".to_string(),
                font_size: 20.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 0.0,
                x_end: 100.0,
            },
            ObjectSpan {
                text: "hello".to_string(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 50.0,
                x_end: 100.0,
            },
        ];
        let result = compute_body_font_size(&spans);
        assert_eq!(result, Some(12.0));
    }

    // --- coalesce_into_lines tests ---

    #[test]
    fn coalesce_groups_by_y_position() {
        let spans = vec![
            ObjectSpan {
                text: "Hello".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 50.0,
            },
            ObjectSpan {
                text: "World".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 55.0,
                y_position: 100.5,
                x_end: 100.0,
            },
            ObjectSpan {
                text: "New line".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 80.0,
                x_end: 80.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello World");
        assert_eq!(lines[1].text, "New line");
    }

    #[test]
    fn coalesce_sorts_by_x_within_line() {
        let spans = vec![
            ObjectSpan {
                text: "World".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 60.0,
                y_position: 100.0,
                x_end: 100.0,
            },
            ObjectSpan {
                text: "Hello".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 50.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello World");
    }

    #[test]
    fn coalesce_no_space_when_overlapping() {
        let spans = vec![
            ObjectSpan {
                text: "Hel".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 30.0,
            },
            ObjectSpan {
                text: "lo".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 28.0,
                y_position: 100.0,
                x_end: 45.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello");
    }

    #[test]
    fn coalesce_no_space_after_hyphen() {
        let spans = vec![
            ObjectSpan {
                text: "self-".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 40.0,
            },
            ObjectSpan {
                text: "aware".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 50.0,
                y_position: 100.0,
                x_end: 90.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "self-aware");
    }

    #[test]
    fn coalesce_bold_majority_rule() {
        let spans = vec![
            ObjectSpan {
                text: "Abc".into(),
                font_size: 12.0,
                is_bold: true,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 30.0,
            },
            ObjectSpan {
                text: "de".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 35.0,
                y_position: 100.0,
                x_end: 50.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].is_bold, "majority of alpha chars are bold");
    }

    #[test]
    fn coalesce_dominant_font_size_weighted_average() {
        let spans = vec![
            ObjectSpan {
                text: "Abc".into(),
                font_size: 24.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 50.0,
            },
            ObjectSpan {
                text: "de".into(),
                font_size: 12.0,
                is_bold: false,
                x_position: 55.0,
                y_position: 100.0,
                x_end: 80.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 1);
        assert!((lines[0].dominant_font_size - 19.2).abs() < 0.01);
    }

    #[test]
    fn coalesce_mixed_font_size_uses_smaller_tolerance() {
        let spans = vec![
            ObjectSpan {
                text: "Small".into(),
                font_size: 6.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 100.0,
                x_end: 30.0,
            },
            ObjectSpan {
                text: "Large".into(),
                font_size: 24.0,
                is_bold: false,
                x_position: 0.0,
                y_position: 96.0,
                x_end: 80.0,
            },
        ];
        let lines = coalesce_into_lines(spans);
        assert_eq!(lines.len(), 2, "should not merge across tolerance boundary");
    }

    // --- is_heading_candidate tests ---

    #[test]
    fn heading_candidate_rejects_too_short() {
        assert!(!is_heading_candidate("ab"));
        assert!(is_heading_candidate("abc"));
    }

    #[test]
    fn heading_candidate_rejects_too_long() {
        let long = "a".repeat(180);
        assert!(is_heading_candidate(&long));
        let too_long = "a".repeat(181);
        assert!(!is_heading_candidate(&too_long));
    }

    #[test]
    fn heading_candidate_rejects_no_alpha() {
        assert!(!is_heading_candidate("12345"));
        assert!(!is_heading_candidate("---..."));
    }

    #[test]
    fn heading_candidate_rejects_low_alpha_density() {
        assert!(!is_heading_candidate("a11111111"));
    }

    #[test]
    fn heading_candidate_rejects_too_many_words() {
        let words: String = (0..21).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        assert!(!is_heading_candidate(&words));
    }

    #[test]
    fn heading_candidate_rejects_prose_punctuation() {
        assert!(!is_heading_candidate("Sentence one. Sentence two. Sentence three."));
    }

    #[test]
    fn heading_candidate_accepts_valid_heading() {
        assert!(is_heading_candidate("Introduction"));
        assert!(is_heading_candidate("Chapter 1: Getting Started"));
    }

    #[test]
    fn heading_candidate_requires_min_three_alpha() {
        assert!(!is_heading_candidate("a 1"));
        assert!(!is_heading_candidate("ab 1"));
        assert!(is_heading_candidate("abc 1"));
    }

    // --- classify_line_headings tests ---

    #[test]
    fn classify_headings_by_ratio() {
        let lines = vec![
            LineSpan {
                text: "Big Title".into(),
                dominant_font_size: 24.0,
                is_bold: false,
                y_position: 700.0,
                x_position: 0.0,
            },
            LineSpan {
                text: "Section Header".into(),
                dominant_font_size: 19.2,
                is_bold: false,
                y_position: 600.0,
                x_position: 0.0,
            },
            LineSpan {
                text: "Subsection".into(),
                dominant_font_size: 14.4,
                is_bold: false,
                y_position: 500.0,
                x_position: 0.0,
            },
            LineSpan {
                text: "Body text that is long enough to be a real paragraph of text.".into(),
                dominant_font_size: 12.0,
                is_bold: false,
                y_position: 400.0,
                x_position: 0.0,
            },
        ];
        let thresholds = HeadingThresholds::default();
        let headings = classify_line_headings(&lines, 12.0, &thresholds);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[2].level, 3);
    }

    #[test]
    fn classify_headings_bold_as_h3() {
        let lines = vec![LineSpan {
            text: "Bold Subhead".into(),
            dominant_font_size: 13.2,
            is_bold: true,
            y_position: 600.0,
            x_position: 0.0,
        }];
        let thresholds = HeadingThresholds::default();
        let headings = classify_line_headings(&lines, 12.0, &thresholds);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].level, 3);
    }

    #[test]
    fn classify_headings_bold_rejected_with_punctuation() {
        let lines = vec![LineSpan {
            text: "Bold sentence.".into(),
            dominant_font_size: 13.2,
            is_bold: true,
            y_position: 600.0,
            x_position: 0.0,
        }];
        let thresholds = HeadingThresholds::default();
        let headings = classify_line_headings(&lines, 12.0, &thresholds);
        assert!(headings.is_empty(), "bold with sentence punctuation rejected");
    }

    #[test]
    fn classify_headings_skips_body_text() {
        let lines = vec![LineSpan {
            text: "Just normal body text here".into(),
            dominant_font_size: 12.0,
            is_bold: false,
            y_position: 400.0,
            x_position: 0.0,
        }];
        let thresholds = HeadingThresholds::default();
        let headings = classify_line_headings(&lines, 12.0, &thresholds);
        assert!(headings.is_empty());
    }

    // --- insert_heading_markers tests ---

    #[test]
    fn insert_markers_basic() {
        let page_text = "Introduction\nBody text here.\nConclusion";
        let headings = vec![
            AcceptedHeading {
                level: 1,
                text: "Introduction".into(),
                y_position: 700.0,
                x_position: 0.0,
            },
            AcceptedHeading {
                level: 2,
                text: "Conclusion".into(),
                y_position: 300.0,
                x_position: 0.0,
            },
        ];
        let result = insert_heading_markers(page_text, &headings);
        assert!(result.contains("\n# Introduction\n"), "H1 marker: {result:?}");
        assert!(result.contains("\n## Conclusion\n"), "H2 marker: {result:?}");
    }

    #[test]
    fn insert_markers_preserves_unmatched_text() {
        let page_text = "Body text that stays the same.";
        let headings = vec![AcceptedHeading {
            level: 1,
            text: "Not In Text".into(),
            y_position: 700.0,
            x_position: 0.0,
        }];
        let result = insert_heading_markers(page_text, &headings);
        assert_eq!(result, page_text, "unmatched heading should leave text unchanged");
    }

    #[test]
    fn insert_markers_duplicate_text_monotonic() {
        let page_text = "Summary\nBody\nSummary";
        let headings = vec![
            AcceptedHeading {
                level: 2,
                text: "Summary".into(),
                y_position: 700.0,
                x_position: 0.0,
            },
            AcceptedHeading {
                level: 3,
                text: "Summary".into(),
                y_position: 300.0,
                x_position: 0.0,
            },
        ];
        let result = insert_heading_markers(page_text, &headings);
        let first = result.find("## Summary");
        let second = result.find("### Summary");
        assert!(first.is_some(), "first match missing: {result:?}");
        assert!(second.is_some(), "second match missing: {result:?}");
        assert!(first < second, "first match should appear before second: {result:?}");
    }

    #[test]
    fn insert_markers_whitespace_normalization_match() {
        let page_text = "  Big   Title  \nBody text.";
        let headings = vec![AcceptedHeading {
            level: 1,
            text: "Big Title".into(),
            y_position: 700.0,
            x_position: 0.0,
        }];
        let result = insert_heading_markers(page_text, &headings);
        assert!(
            result.contains("# Big Title\n") || result.contains("# Big   Title"),
            "should match despite whitespace differences: {result:?}"
        );
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)]
    async fn heading_detection_does_not_affect_non_pdf_extractors() {
        let registry = test_registry();

        let md_input = b"# Existing Heading\n\nBody text";
        let md_result = registry
            .extract(FileType::Markdown, md_input, &ExtractionOptions::default())
            .await
            .expect("markdown extraction");
        assert_eq!(
            md_result.text, "# Existing Heading\n\nBody text",
            "markdown extractor must not alter content"
        );

        let txt_input = b"Plain text with no headings";
        let txt_result = registry
            .extract(FileType::Text, txt_input, &ExtractionOptions::default())
            .await
            .expect("text extraction");
        assert_eq!(
            txt_result.text, "Plain text with no headings",
            "text extractor must not alter content"
        );
    }
}
