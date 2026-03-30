# PDF Heading Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect H1/H2/H3 headings in native PDF pages via font-size heuristics and insert inline markdown markers into extracted text.

**Architecture:** Six-step per-page pipeline (collect object spans, coalesce into lines, compute body font size, classify headings, locate offsets via monotonic matching, insert markers). All logic private to `extract.rs`, no trait/API changes. Downstream markdown chunker consumes markers automatically.

**Tech Stack:** Rust, pdfium-render 0.8, tracing

**Spec:** `docs/superpowers/specs/2026-03-30-pdf-heading-detection-design.md`

**Beads:** apex-sr2

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `crates/rag-core/src/extract.rs` | Modify (lines 280-331) | Add heading detection types, helpers, and integration point |
| `crates/rag-core/src/extract.rs` | Add (new private items) | `ObjectSpan`, `LineSpan`, `HeadingThresholds`, `AcceptedHeading` structs; 7 helper functions |
| `crates/rag-core/tests/fixtures/headings_structured.pdf` | Create | Test fixture: multi-page PDF with known heading typography |
| `crates/rag-core/tests/fixtures/uniform_font.pdf` | Create | Test fixture: single-page PDF with uniform font size |

---

## Task 1: Types and `HeadingThresholds`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing test for `HeadingThresholds::default()`**

Add at the bottom of the `#[cfg(test)] mod tests` block in `extract.rs`:

```rust
#[test]
fn heading_thresholds_default_values() {
    let t = HeadingThresholds::default();
    assert!((t.h1_ratio - 2.0).abs() < f32::EPSILON);
    assert!((t.h2_ratio - 1.6).abs() < f32::EPSILON);
    assert!((t.h3_ratio - 1.2).abs() < f32::EPSILON);
    assert!((t.bold_min_ratio - 1.1).abs() < f32::EPSILON);
    assert!(t.bold_as_h3);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core heading_thresholds_default_values`
Expected: FAIL — `HeadingThresholds` not found

- [ ] **Step 3: Add types and `HeadingThresholds`**

Add above the `impl FormatExtractor for PdfExtractor` block (after the `PdfExtractor::new()` impl block, around line 235):

```rust
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
        Self {
            h1_ratio: 2.0,
            h2_ratio: 1.6,
            h3_ratio: 1.2,
            bold_min_ratio: 1.1,
            bold_as_h3: true,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core heading_thresholds_default_values`
Expected: PASS

- [ ] **Step 5: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add heading detection types and HeadingThresholds (apex-sr2)"
```

---

## Task 2: `normalize_with_offset_map`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-core normalize_with_offset_map`
Expected: FAIL — function not found

- [ ] **Step 3: Implement `normalize_with_offset_map`**

Add after the `HeadingThresholds` impl block:

```rust
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
                // offset_map should have exactly normalized.len() entries
                // after this loop, but we only need to push for bytes
                // beyond what we already have.
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rag-core normalize_with_offset_map`
Expected: PASS

- [ ] **Step 5: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add normalize_with_offset_map helper (apex-sr2)"
```

---

## Task 3: `compute_body_font_size`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
#[test]
fn body_font_size_picks_mode() {
    // 100 alphabetic chars at size 12, 20 at size 24
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
    // Equal alphabetic chars at two sizes
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
    // Winning bucket has 10 alpha chars out of 100 total = 10% < 20%
    let mut spans = vec![ObjectSpan {
        text: "a".repeat(10),
        font_size: 12.0,
        is_bold: false,
        x_position: 0.0,
        y_position: 0.0,
        x_end: 100.0,
    }];
    // Add 9 other buckets each with ~10 alpha chars at different sizes
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
    // "12345" has zero alphabetic chars — should be ignored
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-core body_font_size`
Expected: FAIL — function not found

- [ ] **Step 3: Implement `compute_body_font_size`**

Add after `normalize_with_offset_map`:

```rust
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
    let (best_bucket, best_weight) = buckets
        .into_iter()
        .max_by(|(bucket_a, weight_a), (bucket_b, weight_b)| {
            weight_a.cmp(weight_b).then_with(|| bucket_b.cmp(bucket_a))
        })?;

    // Stability gate: winning bucket must cover >= 20% of total alpha chars.
    if (best_weight as f64) < (total_alpha as f64 * 0.2) {
        return None;
    }

    Some(best_bucket as f32 / 10.0)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rag-core body_font_size`
Expected: PASS

- [ ] **Step 5: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add compute_body_font_size with stability gate (apex-sr2)"
```

---

## Task 4: `coalesce_into_lines`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
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
            y_position: 100.5, // within tolerance (12*0.5=6)
            x_end: 100.0,
        },
        ObjectSpan {
            text: "New line".into(),
            font_size: 12.0,
            is_bold: false,
            x_position: 0.0,
            y_position: 80.0, // different line
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
            x_position: 28.0, // x_position <= prev x_end (30)
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
    // 3 bold alpha chars ("Abc"), 2 non-bold alpha chars ("de")
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
    // "Abc" (3 alpha chars) at 24.0, "de" (2 alpha chars) at 12.0
    // weighted avg = (3*24 + 2*12) / (3+2) = 96/5 = 19.2
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
    // Object at size 6 and object at size 24: tolerance = min(6,24)*0.5 = 3
    // y diff of 4 > 3, so different lines
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
            y_position: 96.0, // diff=4 > tolerance=3
            x_end: 80.0,
        },
    ];
    let lines = coalesce_into_lines(spans);
    assert_eq!(lines.len(), 2, "should not merge across tolerance boundary");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-core coalesce`
Expected: FAIL — function not found

- [ ] **Step 3: Implement `coalesce_into_lines`**

Add after `compute_body_font_size`:

```rust
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
        b.y_position
            .partial_cmp(&a.y_position)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.x_position
                    .partial_cmp(&b.x_position)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut lines: Vec<Vec<ObjectSpan>> = Vec::new();

    for span in spans {
        let mut merged = false;
        for line in &mut lines {
            // Compare against the first object in the line group to decide
            // membership. Use the smaller font size for tolerance.
            let representative = &line[0];
            let tolerance =
                span.font_size.min(representative.font_size) * 0.5;
            if (span.y_position - representative.y_position).abs() < tolerance {
                line.push(span);
                merged = true;
                break;
            }
        }
        if !merged {
            lines.push(vec![span]);
        }
    }

    lines
        .into_iter()
        .map(|mut group| {
            // Sort by x within each line group.
            group.sort_by(|a, b| {
                a.x_position
                    .partial_cmp(&b.x_position)
                    .unwrap_or(std::cmp::Ordering::Equal)
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

                // Spacing: insert space unless overlapping or prev ends with hyphen.
                if !text.is_empty() {
                    let needs_space = !prev_ends_hyphen
                        && prev_x_end
                            .map(|end| span.x_position > end)
                            .unwrap_or(true);
                    if needs_space {
                        text.push(' ');
                    }
                }
                text.push_str(&span.text);
                prev_x_end = Some(span.x_end);
                prev_ends_hyphen = span.text.ends_with('-');
            }

            let dominant_font_size = if total_chars > 0 {
                (weighted_size_sum / total_chars as f64) as f32
            } else {
                0.0
            };

            let is_bold = total_alpha > 0 && bold_alpha * 2 > total_alpha;

            LineSpan {
                text,
                dominant_font_size,
                is_bold,
                y_position: first_y,
                x_position: first_x,
            }
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rag-core coalesce`
Expected: PASS

- [ ] **Step 5: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add coalesce_into_lines with y-clustering (apex-sr2)"
```

---

## Task 5: `is_heading_candidate` and `classify_line_headings`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing tests for `is_heading_candidate`**

Add to the test module:

```rust
#[test]
fn heading_candidate_rejects_too_short() {
    assert!(!is_heading_candidate("ab"));    // 2 chars
    assert!(is_heading_candidate("abc"));    // 3 chars — passes length
}

#[test]
fn heading_candidate_rejects_too_long() {
    let long = "a".repeat(180);
    assert!(is_heading_candidate(&long));    // 180 chars ok
    let too_long = "a".repeat(181);
    assert!(!is_heading_candidate(&too_long)); // 181 rejected
}

#[test]
fn heading_candidate_rejects_no_alpha() {
    assert!(!is_heading_candidate("12345"));
    assert!(!is_heading_candidate("---..."));
}

#[test]
fn heading_candidate_rejects_low_alpha_density() {
    // "a11111111" = 1 alpha / 9 total = 11% < 50%
    assert!(!is_heading_candidate("a11111111"));
}

#[test]
fn heading_candidate_rejects_too_many_words() {
    let words: String =
        (0..21).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
    assert!(!is_heading_candidate(&words)); // 21 words
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
    assert!(!is_heading_candidate("a 1")); // 1 alpha char
    assert!(!is_heading_candidate("ab 1")); // 2 alpha chars
    assert!(is_heading_candidate("abc 1")); // 3 alpha chars
}
```

- [ ] **Step 2: Write failing tests for `classify_line_headings`**

```rust
#[test]
fn classify_headings_by_ratio() {
    let lines = vec![
        LineSpan { text: "Big Title".into(), dominant_font_size: 24.0, is_bold: false, y_position: 700.0, x_position: 0.0 },
        LineSpan { text: "Section Header".into(), dominant_font_size: 19.2, is_bold: false, y_position: 600.0, x_position: 0.0 },
        LineSpan { text: "Subsection".into(), dominant_font_size: 14.4, is_bold: false, y_position: 500.0, x_position: 0.0 },
        LineSpan { text: "Body text that is long enough to be a real paragraph of text.".into(), dominant_font_size: 12.0, is_bold: false, y_position: 400.0, x_position: 0.0 },
    ];
    let thresholds = HeadingThresholds::default();
    let headings = classify_line_headings(&lines, 12.0, &thresholds);
    assert_eq!(headings.len(), 3);
    assert_eq!(headings[0].level, 1); // 24/12 = 2.0
    assert_eq!(headings[1].level, 2); // 19.2/12 = 1.6
    assert_eq!(headings[2].level, 3); // 14.4/12 = 1.2
}

#[test]
fn classify_headings_bold_as_h3() {
    let lines = vec![
        LineSpan { text: "Bold Subhead".into(), dominant_font_size: 13.2, is_bold: true, y_position: 600.0, x_position: 0.0 },
    ];
    let thresholds = HeadingThresholds::default();
    // ratio = 13.2/12 = 1.1, bold, no sentence punctuation → H3
    let headings = classify_line_headings(&lines, 12.0, &thresholds);
    assert_eq!(headings.len(), 1);
    assert_eq!(headings[0].level, 3);
}

#[test]
fn classify_headings_bold_rejected_with_punctuation() {
    let lines = vec![
        LineSpan { text: "Bold sentence.".into(), dominant_font_size: 13.2, is_bold: true, y_position: 600.0, x_position: 0.0 },
    ];
    let thresholds = HeadingThresholds::default();
    let headings = classify_line_headings(&lines, 12.0, &thresholds);
    assert!(headings.is_empty(), "bold with sentence punctuation rejected");
}

#[test]
fn classify_headings_skips_body_text() {
    let lines = vec![
        LineSpan { text: "Just normal body text here".into(), dominant_font_size: 12.0, is_bold: false, y_position: 400.0, x_position: 0.0 },
    ];
    let thresholds = HeadingThresholds::default();
    let headings = classify_line_headings(&lines, 12.0, &thresholds);
    assert!(headings.is_empty());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rag-core heading_candidate && cargo test -p rag-core classify_headings`
Expected: FAIL — functions not found

- [ ] **Step 4: Implement `is_heading_candidate`**

Add after `coalesce_into_lines`:

```rust
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

    // Alphabetic density >= 50%
    if alpha_count * 2 < char_count {
        return false;
    }

    if text.split_whitespace().count() > 20 {
        return false;
    }

    // Reject prose-like text with >= 3 sentence-ending punctuation marks.
    let sentence_ends = text.chars().filter(|&c| c == '.' || c == '?' || c == '!').count();
    if sentence_ends >= 3 {
        return false;
    }

    true
}
```

- [ ] **Step 5: Implement `classify_line_headings`**

Add after `is_heading_candidate`:

```rust
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

        let ratio = line.dominant_font_size / body_size;

        let level = if ratio >= thresholds.h1_ratio {
            Some(1u8)
        } else if ratio >= thresholds.h2_ratio {
            Some(2)
        } else if ratio >= thresholds.h3_ratio {
            Some(3)
        } else if thresholds.bold_as_h3
            && line.is_bold
            && ratio >= thresholds.bold_min_ratio
        {
            // Bold-as-H3 requires zero sentence-ending punctuation.
            let has_sentence_end = line
                .text
                .chars()
                .any(|c| c == '.' || c == '?' || c == '!');
            if has_sentence_end {
                None
            } else {
                Some(3)
            }
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
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rag-core heading_candidate && cargo test -p rag-core classify_headings`
Expected: PASS

- [ ] **Step 7: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 8: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add is_heading_candidate and classify_line_headings (apex-sr2)"
```

---

## Task 6: `insert_heading_markers`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module:

```rust
#[test]
fn insert_markers_basic() {
    let page_text = "Introduction\nBody text here.\nConclusion";
    let headings = vec![
        AcceptedHeading { level: 1, text: "Introduction".into(), y_position: 700.0, x_position: 0.0 },
        AcceptedHeading { level: 2, text: "Conclusion".into(), y_position: 300.0, x_position: 0.0 },
    ];
    let result = insert_heading_markers(page_text, &headings);
    assert!(result.contains("\n# Introduction\n"), "H1 marker: {result:?}");
    assert!(result.contains("\n## Conclusion\n"), "H2 marker: {result:?}");
}

#[test]
fn insert_markers_preserves_unmatched_text() {
    let page_text = "Body text that stays the same.";
    let headings = vec![
        AcceptedHeading { level: 1, text: "Not In Text".into(), y_position: 700.0, x_position: 0.0 },
    ];
    let result = insert_heading_markers(page_text, &headings);
    assert_eq!(result, page_text, "unmatched heading should leave text unchanged");
}

#[test]
fn insert_markers_duplicate_text_monotonic() {
    // Same heading text appears twice; markers should be inserted left-to-right.
    let page_text = "Summary\nBody\nSummary";
    let headings = vec![
        AcceptedHeading { level: 2, text: "Summary".into(), y_position: 700.0, x_position: 0.0 },
        AcceptedHeading { level: 3, text: "Summary".into(), y_position: 300.0, x_position: 0.0 },
    ];
    let result = insert_heading_markers(page_text, &headings);
    // First "Summary" becomes ## Summary, second becomes ### Summary.
    let first = result.find("## Summary");
    let second = result.find("### Summary");
    assert!(first.is_some(), "first match missing: {result:?}");
    assert!(second.is_some(), "second match missing: {result:?}");
    assert!(
        first < second,
        "first match should appear before second: {result:?}"
    );
}

#[test]
fn insert_markers_whitespace_normalization_match() {
    // Page text has extra whitespace; heading text is normalized.
    let page_text = "  Big   Title  \nBody text.";
    let headings = vec![
        AcceptedHeading { level: 1, text: "Big Title".into(), y_position: 700.0, x_position: 0.0 },
    ];
    let result = insert_heading_markers(page_text, &headings);
    assert!(
        result.contains("# Big Title\n") || result.contains("# Big   Title"),
        "should match despite whitespace differences: {result:?}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-core insert_markers`
Expected: FAIL — function not found

- [ ] **Step 3: Implement `insert_heading_markers`**

Add after `classify_line_headings`:

```rust
/// Insert markdown heading markers into page text at positions found by
/// monotonic left-to-right matching. Returns the original text unchanged
/// if no headings could be matched.
fn insert_heading_markers(page_text: &str, headings: &[AcceptedHeading]) -> String {
    if headings.is_empty() {
        return page_text.to_string();
    }

    let (normalized_page, offset_map) = normalize_with_offset_map(page_text);

    // Monotonic matching: find each heading in the normalized page text,
    // always searching forward from where the last match ended.
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

        // Map back to original string positions.
        let orig_start = offset_map[norm_start];
        let orig_end = if norm_end < offset_map.len() {
            offset_map[norm_end]
        } else {
            page_text.len()
        };

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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rag-core insert_markers`
Expected: PASS

- [ ] **Step 5: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add insert_heading_markers with monotonic matching (apex-sr2)"
```

---

## Task 7: `collect_object_spans` and `detect_and_insert_headings`

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Implement `collect_object_spans`**

This function calls PDFium APIs, so it cannot be unit-tested without a real PDF.
Add after the heading detection types block:

```rust
/// Collect text object spans from a PDF page for font analysis.
///
/// Iterates `page.objects()` and extracts text, font size, bold status,
/// and bounding box positions. Uses the `PdfPageObjectCommon` trait for
/// bounds access.
fn collect_object_spans(
    page: &pdfium_render::prelude::PdfPage<'_>,
) -> Vec<ObjectSpan> {
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

        // Get bounding box for position. Fall back to (0,0) if unavailable.
        let (x_position, y_position, x_end) = {
            use pdfium_render::prelude::PdfPageObjectCommon;
            match object.bounds() {
                Ok(bounds) => (
                    bounds.left().value,
                    bounds.bottom().value,
                    bounds.right().value,
                ),
                Err(_) => continue,
            }
        };

        spans.push(ObjectSpan {
            text,
            font_size,
            is_bold,
            x_position,
            y_position,
            x_end,
        });
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
```

- [ ] **Step 2: Implement `detect_and_insert_headings`**

Add after `insert_heading_markers`:

```rust
/// Orchestrator: detect headings on a native-text PDF page and insert
/// inline markdown markers. Returns the original text on any failure or
/// when no headings are detected.
///
/// Implements the six-step pipeline from the spec:
/// 1. Collect object spans
/// 2. Coalesce into line spans
/// 3. Compute body font size
/// 4. Classify line headings
/// 5. Locate offsets and insert markers
// tracing macros internally use .expect()
#[allow(clippy::disallowed_methods)]
fn detect_and_insert_headings(
    page: &pdfium_render::prelude::PdfPage<'_>,
    native_text: &str,
    page_index: usize,
) -> Result<String> {
    let spans = collect_object_spans(page);

    tracing::debug!(
        page_index,
        page_objects_count = spans.len(),
        "collected text object spans"
    );

    if spans.is_empty() {
        return Ok(native_text.to_string());
    }

    let body_size = match compute_body_font_size(&spans) {
        Some(size) => {
            tracing::debug!(page_index, body_font_size = size, "computed body font size");
            size
        }
        None => {
            tracing::debug!(
                page_index,
                "no stable body font size, skipping heading detection"
            );
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
    // Count how many headings actually got inserted by checking for markers.
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
```

- [ ] **Step 3: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean (functions reference existing types/helpers from prior tasks)

- [ ] **Step 4: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): add collect_object_spans and detect_and_insert_headings orchestrator (apex-sr2)"
```

---

## Task 8: Integration into `PdfExtractor::extract()`

**Files:**
- Modify: `crates/rag-core/src/extract.rs` (lines ~280-295)

- [ ] **Step 1: Wire heading detection into the `UseNativeText` arm**

In the `PdfExtractor::extract()` method, find this code inside the page loop:

```rust
                    match page_ocr_decision(settings.force, &native_text, ocr_available) {
                        PageOcrDecision::UseNativeText => {
                            pages.push(native_text);
                        }
```

Replace with:

```rust
                    match page_ocr_decision(settings.force, &native_text, ocr_available) {
                        PageOcrDecision::UseNativeText => {
                            let page_idx = pages.len();
                            let text = detect_and_insert_headings(&page, &native_text, page_idx)
                                .unwrap_or(native_text);
                            pages.push(text);
                        }
```

No changes to the `PerformOcr` or `SkipUnavailable` arms — OCR pages skip heading detection entirely.

- [ ] **Step 2: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 3: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(extract): wire heading detection into PdfExtractor page loop (apex-sr2)"
```

---

## Task 9: Non-PDF regression test

**Files:**
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write non-PDF regression test**

Add to the test module:

```rust
#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn heading_detection_does_not_affect_non_pdf_extractors() {
    let registry = test_registry(); // text + markdown only

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
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p rag-core heading_detection_does_not_affect`
Expected: PASS (this is a regression guard — it should pass immediately)

- [ ] **Step 3: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 4: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "test(extract): add non-PDF regression guard for heading detection (apex-sr2)"
```

---

## Task 10: Test fixtures and integration tests

**Files:**
- Create: `crates/rag-core/tests/fixtures/headings_structured.pdf`
- Create: `crates/rag-core/tests/fixtures/uniform_font.pdf`
- Modify: `crates/rag-core/tests/integration_pdf.rs`

- [ ] **Step 1: Create test fixture PDFs**

Use a Python script to generate the two test PDFs with exact known typography. Run from the project root:

```bash
python3 -c "
from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas
import os

fixtures_dir = 'crates/rag-core/tests/fixtures'

# --- headings_structured.pdf ---
c = canvas.Canvas(os.path.join(fixtures_dir, 'headings_structured.pdf'), pagesize=A4)
w, h = A4

# Page 1: title + section + body + bold subhead
c.setFont('Helvetica-Bold', 24)
c.drawString(72, h - 72, 'Document Title')          # H1: 24pt = 2.0x body

c.setFont('Helvetica', 19.2)
c.drawString(72, h - 130, 'First Section')           # H2: 19.2pt = 1.6x body

c.setFont('Helvetica', 12)
c.drawString(72, h - 170, 'This is body text at the standard twelve point size.')
c.drawString(72, h - 190, 'More body text continues here with additional content.')

c.setFont('Helvetica-Bold', 13.2)
c.drawString(72, h - 240, 'Bold Subheading')         # H3 bold: 13.2pt = 1.1x, bold

c.setFont('Helvetica', 12)
c.drawString(72, h - 280, 'Body text under the bold subheading continues here.')

# Page 2: another section + subsection + body
c.showPage()
c.setFont('Helvetica', 19.2)
c.drawString(72, h - 72, 'Second Section')            # H2

c.setFont('Helvetica', 14.4)
c.drawString(72, h - 120, 'Subsection Here')          # H3: 14.4pt = 1.2x body

c.setFont('Helvetica', 12)
c.drawString(72, h - 160, 'Body text in the second section with enough content.')
c.drawString(72, h - 180, 'Additional body text for the second section area.')

c.save()
print('Created headings_structured.pdf')

# --- uniform_font.pdf ---
c = canvas.Canvas(os.path.join(fixtures_dir, 'uniform_font.pdf'), pagesize=A4)
c.setFont('Helvetica', 12)
c.drawString(72, h - 72, 'All text at same size')
c.drawString(72, h - 92, 'No heading signals here')
c.drawString(72, h - 112, 'Everything is twelve point')
c.drawString(72, h - 132, 'The body font size histogram should show uniform distribution')
c.save()
print('Created uniform_font.pdf')
"
```

If reportlab is not installed, run `uv pip install reportlab` first, or use any other PDF generation method that produces the same typography.

- [ ] **Step 2: Verify fixtures exist**

Run: `ls -la crates/rag-core/tests/fixtures/headings_structured.pdf crates/rag-core/tests/fixtures/uniform_font.pdf`
Expected: both files present

- [ ] **Step 3: Write integration tests**

Add to `crates/rag-core/tests/integration_pdf.rs`:

```rust
#[tokio::test]
#[ignore] // requires PDFIUM_LIBRARY_PATH
#[allow(clippy::disallowed_methods)]
async fn pdf_heading_detection_inserts_markers() {
    let config = match test_pdf_config() {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set");
            return;
        }
    };

    let extractor = rag_core::extract::PdfExtractor::new(&config)
        .expect("PdfExtractor should construct");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/headings_structured.pdf");
    let pdf_bytes = std::fs::read(&fixture).expect("reading fixture");

    let result = extractor
        .extract(&pdf_bytes, &rag_core::extract::ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    // Verify heading markers were inserted.
    assert!(
        result.text.contains("# Document Title"),
        "should contain H1 marker, got:\n{}",
        &result.text[..result.text.len().min(500)]
    );
    assert!(
        result.text.contains("## First Section") || result.text.contains("## Second Section"),
        "should contain H2 marker, got:\n{}",
        &result.text[..result.text.len().min(500)]
    );
}

#[tokio::test]
#[ignore] // requires PDFIUM_LIBRARY_PATH + Tesseract
#[allow(clippy::disallowed_methods)]
async fn pdf_heading_detection_ocr_no_markers() {
    let config = match test_pdf_config() {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set");
            return;
        }
    };

    let extractor = rag_core::extract::PdfExtractor::new(&config)
        .expect("PdfExtractor should construct");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hello_ocr.pdf");
    if !fixture.exists() {
        eprintln!("skipping: hello_ocr.pdf fixture not found");
        return;
    }
    let pdf_bytes = std::fs::read(&fixture).expect("reading fixture");

    let result = extractor
        .extract(&pdf_bytes, &rag_core::extract::ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    // OCR pages should have no heading markers.
    assert!(
        !result.text.contains("\n# "),
        "OCR text should not contain heading markers, got:\n{}",
        &result.text[..result.text.len().min(500)]
    );
}

#[tokio::test]
#[ignore] // requires PDFIUM_LIBRARY_PATH
#[allow(clippy::disallowed_methods)]
async fn pdf_heading_detection_no_signal_fallback() {
    let config = match test_pdf_config() {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set");
            return;
        }
    };

    let extractor = rag_core::extract::PdfExtractor::new(&config)
        .expect("PdfExtractor should construct");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/uniform_font.pdf");
    let pdf_bytes = std::fs::read(&fixture).expect("reading fixture");

    let result = extractor
        .extract(&pdf_bytes, &rag_core::extract::ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    // No heading markers should be inserted when all text is same size.
    assert!(
        !result.text.contains("\n# "),
        "uniform-font text should not contain heading markers, got:\n{}",
        &result.text[..result.text.len().min(500)]
    );

    // Text content should still be present.
    assert!(
        result.text.contains("All text at same size"),
        "extracted text should contain fixture content"
    );
}
```

Note: The `test_pdf_config()` helper already exists in the integration test file. Check its signature — if it takes a `tessdata_dir` argument, use `test_pdf_config(None)` for non-OCR tests.

- [ ] **Step 4: Run `cargo fmt --all` and `cargo check`**

Run: `cargo fmt --all && cargo check`
Expected: clean

- [ ] **Step 5: Commit**

```bash
git add crates/rag-core/tests/fixtures/headings_structured.pdf crates/rag-core/tests/fixtures/uniform_font.pdf crates/rag-core/tests/integration_pdf.rs
git commit -m "test(extract): add PDF heading detection integration tests and fixtures (apex-sr2)"
```

---

## Task 11: Verify all tests pass and finalize

- [ ] **Step 1: Run unit tests**

Run: `cargo test -p rag-core -- --skip ignored`
Expected: all new and existing unit tests PASS

- [ ] **Step 2: Run integration tests (if PDFIUM_LIBRARY_PATH is set)**

Run: `cargo test -p rag-core -- --ignored`
Expected: heading detection integration tests PASS

- [ ] **Step 3: Run full workspace check**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean, no warnings

- [ ] **Step 4: Update beads**

Run: `bd update sr2 --status done`

- [ ] **Step 5: Final commit if any adjustments were needed**

```bash
git add -u
git commit -m "fix(extract): address test/lint issues from heading detection (apex-sr2)"
```
