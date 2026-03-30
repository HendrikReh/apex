# PDF Heading Detection via Font-Size Heuristics

**Issue:** apex-sr2 | **Priority:** P3 | **Type:** feature
**Status:** Design approved, ready for implementation planning

## Summary

Layer heading detection (H1/H2/H3) onto the existing `PdfExtractor` using
font-size ratios from PDFium text objects. Accepted headings are inserted as
inline markdown markers (`# `, `## `, `### `) into the extracted text, which
the downstream markdown chunker already understands. No trait changes, no new
crates, no changes to downstream systems.

## Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Output format | Inline markdown markers | Downstream markdown chunker handles `#` markers with zero integration work |
| Thresholds | Hardcoded defaults in `HeadingThresholds::default()` | Configurability deferred until real corpus failures justify it |
| Bold-as-H3 | Included at stricter 1.1x ratio (vs projectAlpha 1.05x) | Reduces false positives while still catching bold sub-headings |
| OCR pages | No heading detection | No font-size signal available from rasterized text |
| Approach | Two-phase with offset tracking (Approach B) | Deterministic offset-based insertion avoids substring-search fragility |
| Granularity | Line-level, not object-level | PDFs split headings across multiple text objects; coalescing prevents missed/duplicate markers |

## Algorithm

Six-step pipeline, computed independently per native-text page. No cross-page
baseline.

### Step 1: Collect Object Spans

Iterate `page.objects()`. For each text object with finite font size > 0,
collect:

```rust
struct ObjectSpan {
    text: String,
    font_size: f32,
    is_bold: bool,
    x_position: f32,
    y_position: f32,
    x_end: f32, // right edge of bounding box, used for inter-object gap detection
}
```

### Step 2: Coalesce into Line Spans

Group objects into lines by y-position clustering. Two objects belong to the
same line if their y-positions differ by less than
`min(font_size_a, font_size_b) * 0.5` (using the smaller font size to prevent
merging across nearby lines of different sizes).

Within a line, sort by x-position ascending. Concatenate text with a single
space inserted between consecutive objects *unless* the previous object's text
ends with a hyphen or the gap between the previous object's `x_end` and the
next object's `x_position` is zero or negative (direct concatenation).

Result:

```rust
struct LineSpan {
    text: String,           // coalesced, normalized
    dominant_font_size: f32, // character-weighted average of constituent objects
    is_bold: bool,          // majority of alphabetic characters from bold objects
    y_position: f32,        // from first object after sorting
    x_position: f32,        // from first object after sorting
}
```

### Step 3: Compute Body Font Size

Histogram mode over all object spans. Both the histogram weighting and the
stability gate use **alphabetic character count** consistently (digits,
punctuation, and symbols are excluded from both calculations):

- Bucket key: `(font_size * 10.0).round() as i32`
- Weight: alphabetic character count of the object span
- Select bucket with highest total weight
- Tie-break: smaller font size wins
- **Stability gate:** winning bucket must account for >= 20% of total
  alphabetic characters on the page; otherwise return page text unchanged
  with no headings
- No usable samples: same fallback

### Step 4: Classify Line Spans

**Candidate filters** (all must pass):

- 3-180 characters (character count, not bytes)
- At least 3 alphabetic characters
- Alphabetic density >= 50%
- At most 20 words
- General prose rejection: fewer than 3 sentence-ending punctuation marks
  (`.`, `?`, `!`)

**Classification by ratio** (`dominant_font_size / body_size`):

| Level | Condition |
|---|---|
| H1 | ratio >= 2.0 |
| H2 | ratio >= 1.6 |
| H3 | ratio >= 1.2 |
| H3 (bold fallback) | ratio >= 1.1 AND line is bold AND zero sentence-ending punctuation |

### Step 5: Locate Offsets via Monotonic Matching

**Normalization contract:** Both the candidate line text and the
`page.text().all()` output are compared using the same normalization: trim
leading/trailing whitespace, collapse all internal whitespace runs (spaces,
tabs, newlines) to a single ASCII space (`0x20`), preserve case and
punctuation unchanged.

A first-class helper performs this normalization while building an offset map:

```rust
fn normalize_with_offset_map(text: &str) -> (String, Vec<usize>)
```

where `vec[normalized_byte_pos] = original_byte_pos`. This is used by
`insert_heading_markers()` to resolve normalized match positions back to
original string positions safely.

**Matching procedure:**

1. Order accepted candidates in reading order: sort by y-position (descending
   in PDF coordinates = top-to-bottom visually), then x-position ascending for
   lines within the same y-tolerance group.
2. Match each candidate's normalized text against the normalized
   `page.text().all()` sequentially. Each search starts where the previous
   match ended, never earlier.
3. If a candidate cannot be matched: skip it, log at debug level, continue
   with remaining candidates.
4. Once all offsets are resolved, map them back to original string positions
   via the offset map.
5. Insert `\n# ` / `\n## ` / `\n### ` markers in reverse byte-offset order.

### Step 6: OCR Pages

Skip heading detection entirely. Return plain text, no markers. Native PDF
metadata (if any) is still carried through.

### Offset Contract

All offsets are derived from and applied to the single `page.text().all()`
string. Object text is used only for font analysis and coalescing, never for
offset arithmetic.

### Observability

Debug-level tracing on every page (not user-configurable):

- `page_index` -- which page
- `page_objects_count` -- number of native text objects
- `body_font_size` -- chosen body size and its character coverage percentage
- `headings_accepted` -- number of heading markers inserted
- `candidates_skipped` -- number of candidates that failed monotonic matching

## Integration

### Where It Slots In

Inside `PdfExtractor::extract()`, in the `spawn_blocking` closure, in the
`PageOcrDecision::UseNativeText` arm. The caller already has `native_text`
from `page.text().all()`.

```
UseNativeText => {
    let text = detect_and_insert_headings(&page, &native_text, page_index)
        .unwrap_or_else(|_| native_text);
    pages.push(text);
}
```

### Function Signature

```rust
fn detect_and_insert_headings(
    page: &PdfPage<'_>,
    native_text: &str,
    page_index: usize,
) -> Result<String>
```

Falls back to `native_text` on any error. Does not call `page.text().all()`
again -- uses the already-extracted text.

### New Types and Helpers (all private to `extract.rs`)

**Types:** `ObjectSpan`, `LineSpan`, `HeadingThresholds`, `AcceptedHeading`

**Helpers:**

- `collect_object_spans(page) -> Vec<ObjectSpan>`
- `coalesce_into_lines(spans) -> Vec<LineSpan>`
- `compute_body_font_size(spans) -> Option<f32>` (returns `None` when
  stability gate fails)
- `classify_line_headings(lines, body_size, thresholds) -> Vec<AcceptedHeading>`
- `normalize_with_offset_map(text) -> (String, Vec<usize>)`
- `insert_heading_markers(page_text, headings) -> String` (monotonic match +
  reverse insert)
- `detect_and_insert_headings(page, native_text, page_index) -> Result<String>`
  (orchestrator)

### What Does NOT Change

- `ExtractionResult` -- still returns inline-transformed text only
- `FormatExtractor` trait -- no signature change
- `ExtractionOptions` -- no new fields
- `ExtractorRegistry` -- unchanged
- `rag-chunking` -- markdown chunker already handles `#` markers
- Non-PDF extractors -- unaffected

## Testing Strategy

### Unit Tests (no PDFium required, run in CI)

| Test | What it covers |
|---|---|
| `compute_body_font_size` | Histogram mode, tie-break to smaller size, stability gate (<20% returns None) |
| `coalesce_into_lines` | Y-clustering with tolerance, x-sorting, space insertion, bold majority rule |
| `classify_line_headings` | Ratio thresholds, bold-as-H3 at 1.1x, punctuation guards, alphabetic density/count filters |
| `normalize_with_offset_map` | Whitespace collapsing, offset map accuracy, round-trip to original positions |
| `normalize_mismatch_tolerance` | Source with tabs/newlines/multi-spaces, candidate with collapsed spaces, offset map resolves correctly |
| `insert_heading_markers` | Reverse offset insertion, monotonic match skipping on no-match, marker format |
| `is_heading_candidate` | Boundary: 2 chars rejected, 3 accepted, 180 accepted, 181 rejected, digit-dominated, low alpha density |
| `duplicate_text_monotonic` | Same normalized line appears twice on page, matches resolve left-to-right deterministically |
| `non_pdf_unchanged` | Markdown and text extractors produce unchanged output (guards against heading logic leaking) |

### Integration Tests (require `PDFIUM_LIBRARY_PATH`, `#[ignore]` in CI)

| Test | What it covers |
|---|---|
| `pdf_heading_detection_inserts_markers` | Load fixture PDF with known headings, verify `#`/`##`/`###` at expected positions |
| `pdf_heading_detection_ocr_no_markers` | OCR-only fixture, verify no markers inserted |
| `pdf_heading_detection_no_signal_fallback` | Uniform font-size fixture, verify exact text equality with non-heading extraction |

### Test Fixtures

| Fixture | Purpose | Notes |
|---|---|---|
| `headings_structured.pdf` (new) | Heading insertion test | 2-3 pages, large title, medium section headers, body text, one bold sub-heading. Purpose-built with known typography. |
| `hello_ocr.pdf` (existing) | OCR-no-markers test | Scanned page, already in `tests/fixtures/`. Verifies heading detection is skipped for OCR pages. |
| `uniform_font.pdf` (new) | No-signal fallback test | Single page, all text at same font size. Verifies exact text equality with non-heading extraction. |

All fixtures committed to `tests/fixtures/`.

## Scope Boundaries

### In Scope

- Six-step heading detection pipeline in `PdfExtractor`
- `HeadingThresholds::default()` with hardcoded ratios
- `normalize_with_offset_map` helper
- Private types and helpers in `extract.rs`
- Unit tests for all pure functions
- Integration tests with purpose-built fixture
- Debug-level tracing for observability
- Best-effort heading insertion; extraction must not fail if
  heuristics/matching fail

### Out of Scope

- Configurable thresholds via `app.toml` (follow-up if needed)
- Heading metadata struct / `HeadingInfo` return type (follow-up apex issue)
- OCR-based heading detection
- Cross-page baseline or document-level font analysis
- Reimplementation of PDF text extraction (augments `page.text().all()`, does
  not replace PDFium's text assembly)
- Changes to `ExtractionResult`, `FormatExtractor`, `ExtractionOptions`, or
  `ExtractorRegistry`
- Changes to `rag-chunking`

## Reference

- **projectAlpha implementation:**
  `/Users/hendrik/Developer/projectAlpha/crates/rag-core/src/extract.rs`
  (lines 49-328)
- **Apex PDF extractor:**
  `/Users/hendrik/Developer/apex/crates/rag-core/src/extract.rs`
  (`PdfExtractor` at lines 172-337)
- **Markdown chunker:**
  `/Users/hendrik/Developer/apex/crates/rag-chunking/src/chunking_markdown.rs`
