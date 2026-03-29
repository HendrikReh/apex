# Code Reviewer

Review code changes for adherence to project conventions and architecture rules.

## What to Check

### Error Handling (ADR-001)
- No `.unwrap()` or `.expect()` in production code — enforced by `clippy.toml`
- Exception: `tracing` macros and `serde_json::json!` internally use `.expect()` — require a per-function `#[allow(clippy::disallowed_methods)]` with a comment explaining why
- `.expect_err()` is allowed — it is NOT in the disallowed list
- Errors propagated with `?` and context added via `anyhow::Context`
- Do NOT add `#[must_use]` to functions returning `Result` — it's already implied
- Module-level `#![allow(clippy::disallowed_methods)]` is forbidden — use per-function only

### Crate Boundaries
- **Library crates** (`rag-core`, `rag-chunking`, `agent-core`, `rag-client`, `rag-evidence`, `rag-notifications`, `rag-sbom`): Must NOT write to stdout/stderr or call `std::process::exit`. All errors as `Result` types.
- **Binary crates** (`rag-server`, `rag-cli`): May write to console.
- Dependency flow:
  - `rag-cli -> rag-client -> reqwest` (ONLY)
  - `rag-cli -> rag-evidence`
  - `rag-server -> rag-core -> rag-chunking`
  - `rag-server -> agent-core` (post-MVP)
  - `rag-server -> rag-evidence` (post-MVP)
  - `rag-server -> rag-notifications` (post-MVP)
  - `rag-chunking`: no workspace deps (pure logic + tiktoken-rs)
  - `rag-client`: no workspace deps (reqwest only)
  - `obfuscate-macros`: no workspace deps (proc-macro)
  - `test-support`: no workspace deps
- `test-support`: Shared test utilities — do NOT add production logic here

### Document Identity (ADR-002)
- Document identity is `(tenant, document_id)` — collection is routing metadata, not identity
- All queries must filter by `TenantId` — multi-tenancy is a day-1 requirement
- Default tenant: `"default"`

### Workspace Dependencies
- All shared dependencies use `{ workspace = true }` in crate `Cargo.toml` files
- Version pins belong only in the root `[workspace.dependencies]` section
- Check for duplicate version specifications in crate-level `Cargo.toml`

### Axum Conventions
- Routes use `:param` syntax (e.g., `/documents/:document_id`), NOT `{param}` (that's axum 0.8+)
- Handlers return `impl IntoResponse` or typed `Result<Json<T>, AppError>`
- Use `axum::extract::State` for shared state, not custom extractors

### Serde Conventions
- Optional fields in request/response types: use `#[serde(skip_serializing_if = "Option::is_none")]` for requests, `#[serde(default)]` for responses
- Keep serialization compact — don't send `null` fields unnecessarily

### Formatting
- `max_width = 100`, Unix newlines, `reorder_imports = true`, edition 2024
- All crate `lib.rs` files include `#![deny(unsafe_code)]` — new crates must add this

### Configuration
- Two-tier: `config/app.toml` for non-secrets, `.env` for secrets only
- Precedence: env vars > `config/app.toml` > hardcoded defaults
- Never hardcode values that belong in config (ports, URLs, feature flags)

### Test Patterns
- HTTP-level CLI tests: mock `axum::Router` + `test_support::spawn_app` for ephemeral port
- State capture: `Arc<Mutex<Option<T>>>` for request bodies in mock handlers
- Test functions using only `.expect_err()` do NOT need `#[allow(clippy::disallowed_methods)]`
- Tests go in `#[cfg(test)]` modules or dedicated test files — not mixed into production code
- Use mock embedder for all automated tests (no API key required)

## Process

1. Identify all changed files (use `git diff` or review the provided diff)
2. Categorize by crate type (library vs binary) and subsystem (server/core/CLI)
3. Check each file against the rules above
4. Flag violations with file path, line number, rule violated, and suggested fix
5. Classify findings:
   - **Blocking**: Must fix — `.unwrap()` in lib crates, crate boundary violations, missing tenant filtering
   - **Warning**: Should fix — missing `serde` annotations, weak validation, inconsistent error messages
   - **Suggestion**: Nice to have — naming, docs, performance
6. Summarize findings by severity
