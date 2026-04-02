# Testing Guide

This repo uses three test tiers:

- `unit-test`: fast crate-local tests with no Docker, provider, or native-library requirements
- `integration-test`: tests that exercise real Postgres/Qdrant, HTTP routes, and end-to-end application wiring
- `smoke-test`: optional higher-cost tests that need external providers or native tools such as PDFium or Tesseract
- `agentic-eval`: offline routed-search benchmark for `agentic_search_v1`

The commands below are the supported entry points:

```bash
just unit-test
just integration-test
just smoke-test
just agentic-eval
just test
```

`just test` remains the default CI-style command:

- runs `cargo fmt --check`
- runs strict `clippy`
- runs non-ignored `cargo test --workspace`

It does not run ignored integration or smoke tests.

## Contents

- [Command Reference](#command-reference)
- [just test](#just-test)
- [Test Taxonomy](#test-taxonomy)
- [Writing New Tests](#writing-new-tests)
- [Execution Workflow](#execution-workflow)
- [Recommended Development Flow](#recommended-development-flow)
- [Current Mapping In This Repo](#current-mapping-in-this-repo)
- [Troubleshooting](#troubleshooting)

## Command Reference

### `just test`

Use this as the default pre-push quality gate.

Runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings -D clippy::disallowed_methods
cargo test --workspace
```

This is broader than the other tiered commands:

- includes formatting and lint checks
- runs the full non-ignored workspace test suite
- does not run ignored integration or smoke tests

### `just unit-test`

Use this for fast feedback while writing code.

Runs:

```bash
cargo test --workspace --lib --bins
```

Typical coverage:

- `#[cfg(test)]` modules inside `src/*.rs`
- binary-crate unit tests such as `crates/rag-cli/src/main.rs`

Good use cases:

- validating pure functions
- config parsing logic
- request validation logic
- formatter/output helpers

### `just integration-test`

Use this for real application behavior with Docker-backed infra.

This command starts local services first:

```bash
just up
```

Then it runs the repo's integration targets, including:

- `crates/rag-client/tests/client_tests.rs`
- `crates/rag-cli/tests/e2e.rs`
- `crates/rag-server/tests/health.rs`
- `crates/rag-server/tests/ingest.rs`
- `crates/rag-server/tests/search.rs`
- `crates/rag-server/tests/e2e.rs`
- `crates/rag-core/tests/integration_conversations.rs`
- `crates/rag-core/tests/integration_ingest.rs`
- `crates/rag-core/tests/integration_lifecycle.rs`
- `crates/rag-core/tests/integration_retrieval.rs`

These cover behavior such as:

- ingesting real documents into Postgres + Qdrant
- invoking `rag-cli` as a subprocess against a live app
- HTTP request/response behavior through Axum
- end-to-end API journeys
- tenant isolation through the API

### `just smoke-test`

Use this for optional environment-specific verification.

This command currently runs:

- `crates/rag-core/tests/integration_pdf.rs`
- `crates/rag-core/tests/integration_chat.rs`
- `crates/rag-core/tests/smoke_llm.rs`

These tests require some combination of:

- `just up`
- `PDFIUM_LIBRARY_PATH`
- `TESSDATA_PREFIX`
- `LLM_PROVIDER`
- `LLM_API_KEY`

They are intentionally `#[ignore]` because they are slower, costlier, or depend on local machine setup.

### `just agentic-eval`

Use this to compare the baseline `/chat` path against `agentic_search_v1` on the checked-in benchmark set under `data/evals/agentic_search_v1/`.

Runs:

```bash
just up
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agentic_eval -- --ignored --nocapture --test-threads=1
```

This is an ignored integration benchmark, so it exercises the live server stack without running in the default `just test` flow.

## Test Taxonomy

### Unit tests

Put unit tests in the same file as the code under test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_input() {
        assert!(validate("").is_err());
    }
}
```

Use unit tests for:

- branchy logic
- validation rules
- error classification
- formatting
- helper functions with no external dependencies

### Integration tests

Put integration tests under `crates/<crate>/tests/`.

Examples in this repo:

- [integration_ingest.rs](/Users/hendrik/Developer/apex/crates/rag-core/tests/integration_ingest.rs)
- [e2e.rs](/Users/hendrik/Developer/apex/crates/rag-server/tests/e2e.rs)
- [client_tests.rs](/Users/hendrik/Developer/apex/crates/rag-client/tests/client_tests.rs)

Use integration tests for:

- real `AppConfig -> Stores -> Service` wiring
- Postgres/Qdrant behavior
- HTTP routing and middleware
- cross-module workflows

### Smoke tests

Use smoke tests for behavior that is valuable to verify, but not appropriate for every default run.

Examples:

- real LLM provider calls
- PDFium-backed PDF extraction
- Tesseract OCR fallback

Mark them ignored:

```rust
#[tokio::test]
#[ignore] // requires PDFium + Tesseract installed
async fn pdf_extractor_ocr_fallback_on_scanned_page() {
    // ...
}
```

## Writing New Tests

### General rules

- Prefer unit tests first.
- Use integration tests when the behavior crosses module or storage boundaries.
- Use smoke tests only when the test depends on expensive or non-portable setup.
- Keep tests deterministic.
- Use unique tenant/collection names in infra-backed tests.
- Prefer mock embedder and mock LLM paths when the test is not specifically about provider behavior.

### Naming

Use behavior-oriented names:

- `reingest_unchanged_file_is_skipped`
- `tenant_isolation_through_api`
- `forced_ocr_without_tessdata_returns_error`

Avoid vague names like:

- `test_ingest`
- `works`

### Isolated data

For infra-backed tests, generate unique suffixes:

```rust
fn unique_suffix() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}
```

Then derive:

- tenant IDs
- collection names
- document IDs when needed

This avoids cross-test interference.

### Environment-sensitive tests

If a test depends on local tools or env vars, gate it explicitly and skip with a clear message:

```rust
let Some(pdfium_path) = std::env::var("PDFIUM_LIBRARY_PATH").ok() else {
    eprintln!("skipping: PDFIUM_LIBRARY_PATH not set");
    return;
};
```

Use this pattern for:

- `PDFIUM_LIBRARY_PATH`
- `TESSDATA_PREFIX`
- provider credentials

## Execution Workflow

### Fast local loop

When changing pure Rust logic:

```bash
just unit-test
```

To run one specific test while iterating:

```bash
cargo test -p rag-core resolve_ocr_settings -- --nocapture
cargo test -p rag-cli ingest_human_output -- --nocapture
```

### Infra-backed verification

When changing ingest, retrieval, stores, or server routes:

```bash
just integration-test
```

To run a specific ignored integration test:

```bash
just up
cargo test -p rag-cli --test e2e ingest_then_chat_via_cli_subprocess -- --ignored --nocapture
cargo test -p rag-core --test integration_ingest reingest_modified_file_updates_chunks_in_place -- --ignored --nocapture
cargo test -p rag-server --test e2e tenant_isolation_through_api -- --ignored --nocapture
```

### Provider/native verification

When changing OCR, PDF extraction, or LLM integrations:

```bash
just smoke-test
```

Targeted examples:

```bash
cargo test -p rag-core --test integration_pdf pdf_extractor_ocr_fallback_on_scanned_page -- --ignored --nocapture
cargo test -p rag-core --test smoke_llm openai_compatible_smoke_test -- --ignored --nocapture
cargo test -p rag-core --test integration_chat chat_returns_answer_and_citations -- --ignored --nocapture
```

## Recommended Development Flow

For a normal feature:

1. Write or update a unit test first if the logic is local.
2. Run `just unit-test`.
3. If the change touches real storage or HTTP behavior, run `just integration-test`.
4. If the change touches providers or native tooling, run `just smoke-test` when your environment is configured.
5. Before pushing, run at least:

```bash
just test
```

## Current Mapping In This Repo

### Unit-test tier

Examples:

- config parsing in `rag-core`
- CLI output and command logic in `rag-cli`
- retrieval/context helper logic

### Integration-test tier

Examples:

- `rag-cli/tests/e2e.rs`
- `rag-core/tests/integration_ingest.rs`
- `rag-core/tests/integration_retrieval.rs`
- `rag-server/tests/e2e.rs`
- `rag-server/tests/search.rs`

### Smoke-test tier

Examples:

- `rag-core/tests/integration_pdf.rs`
- `rag-core/tests/integration_chat.rs`
- `rag-core/tests/smoke_llm.rs`

## Troubleshooting

### Docker-backed tests fail immediately

Check services:

```bash
just up
docker compose ps
```

### Ignored tests do not run

Make sure you included `-- --ignored` when using raw `cargo test`.

### OCR tests skip unexpectedly

Check:

```bash
echo "$PDFIUM_LIBRARY_PATH"
echo "$TESSDATA_PREFIX"
which tesseract
```

### LLM smoke tests skip or fail

Check:

```bash
echo "$LLM_PROVIDER"
echo "$LLM_API_KEY"
```

For Anthropic vs OpenAI-compatible paths, ensure the configured provider matches the test you are trying to run.
