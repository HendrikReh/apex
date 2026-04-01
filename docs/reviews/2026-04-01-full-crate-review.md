# Consolidated Crate Review Report

**Date:** 2026-04-01  
**Branch Reviewed:** `fix/spec-hardening`  
**Scope:** All 12 workspace crates  
**Sources consolidated:** automated multi-agent workspace review + manual Codex sequential review

## Review Method

This document consolidates two review passes:

- a broad automated workspace review that covered architecture, code quality, and security
- a narrower manual Codex pass that re-checked crates sequentially in dependency order

The Codex pass used this review order, derived from local crate dependencies:

1. `obfuscate-macros`
2. `rag-evidence`
3. `rag-license`
4. `rag-notifications`
5. `rag-sbom`
6. `test-support`
7. `agent-core`
8. `rag-chunking`
9. `rag-client`
10. `rag-core`
11. `rag-server`
12. `rag-cli`

## How To Read This Report

Each finding is tagged with one of:

- `Verified`: directly re-checked in code during the manual Codex pass
- `Review`: retained from the broader review because it is plausible and useful, but not fully re-verified line by line in the second pass

That keeps one report while still separating confidence levels.

## Summary

The highest-risk issues remain concentrated in `rag-server`.

Confirmed high-priority problems:
- `rag-server` authz fails open for unmapped routes
- `rag-server` leaks internal error details in HTTP responses
- `rag-server` allows unrestricted server-side ingestion by path
- `rag-core` handles some secrets as plain strings while `AppConfig` derives `Debug`
- `agent-core` has a schema-vs-serde contract mismatch
- `rag-client` masks response-body read failures

Six crates remain effectively clean for the reviewed scope:
- `obfuscate-macros`
- `rag-evidence`
- `rag-license`
- `rag-notifications`
- `rag-sbom`
- `test-support`

## Scorecard

This table preserves the broader workspace review counts as the best available aggregate summary.

| Crate | Level | Blocking | Warning | Suggestion |
|-------|-------|----------|---------|------------|
| obfuscate-macros | 0 | -- | -- | -- |
| rag-evidence | 0 | -- | -- | -- |
| rag-license | 0 | -- | -- | -- |
| rag-notifications | 0 | -- | -- | -- |
| rag-sbom | 0 | -- | -- | -- |
| test-support | 0 | -- | -- | -- |
| **rag-chunking** | 0 | 0 | 4 | 7 |
| **rag-client** | 0 | 2 | 4 | 5 |
| **agent-core** | 0 | 1 | 4 | 6 |
| **rag-core** | 1 | 1 | 7 | 6 |
| **rag-server** | 2 | **4** | 8 | 7 |
| **rag-cli** | 2 | 0 | 4 | 5 |
| **TOTAL** | | **8** | **27** | **36** |

## Cross-Cutting Themes

### 1. Missing `#![deny(unsafe_code)]` in library crates

`Review`: `rag-client`, `agent-core`, `rag-core`, and `rag-server` all lack `#![deny(unsafe_code)]` in `lib.rs`, while `rag-chunking` already enforces it. This is low-effort hardening and should be fixed consistently.

### 2. Internal error leakage to HTTP clients

`Verified`: `rag-server` exposes raw backend errors in several HTTP response paths. This is the most important security issue in the workspace because it leaks SQL, filesystem, and infrastructure details directly to clients.

### 3. `rustfmt.toml` edition mismatch

`Verified`: [`rustfmt.toml`](../../rustfmt.toml) still declares Rust 2021 while the workspace uses Rust 2024.

### 4. Secret handling inconsistencies

`Verified`: `rag-core` still stores some secrets as plain strings even though `llm_api_key` already uses `SecretString`.

### 5. Unrestricted file access in ingestion flows

`Verified`: path-based ingestion is not restricted to an approved base directory. This affects both `rag-server` route exposure and `rag-core` ingestion internals.

### 6. Silent failure masking

`Verified` in `rag-client`, `Review` elsewhere: some fallback patterns hide real failures and reduce diagnosability.

## Findings By Crate

### obfuscate-macros

No material findings in either pass.

### rag-evidence

No material findings in either pass.

### rag-license

No material findings in either pass.

### rag-notifications

No material findings in either pass.

### rag-sbom

No material findings in either pass.

### test-support

No material findings in either pass.

### agent-core

#### Blocking

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `lib.rs` | Missing `#![deny(unsafe_code)]`. |

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `runtime/graph_flow/mod.rs:181`, `spec.rs:720-726` | Existing Clippy issues: `for_kv_map` and `collapsible_if`. |
| `Verified` | `spec.rs:38` vs `agent_spec.schema.json:8-10` | The schema allows top-level `x_*` keys, but the Rust struct uses `deny_unknown_fields`. The published contract is inconsistent. |
| `Review` | `runtime/graph_flow/mod.rs:218` | `unwrap_or_default()` on a logically required `condition_key` silently degrades if the invariant is violated. |
| `Review` | `spec.rs:501-506` | `from_yaml_str_unvalidated` is public and lightly exercised. |

#### Suggestions

| Tag | Finding |
|-----|---------|
| `Review` | `AgentRunConfig` could derive `Serialize` and `Deserialize` for consistency. |
| `Review` | `#[allow(dead_code)]` on the entire `RunMeta` struct is broader than needed. |
| `Review` | Add a direct unit test for `from_yaml_file`. |
| `Review` | `task_id` capture in `execute_loop` may become stale after execution advances. |
| `Review` | Consider `tracing::instrument` on `start`, `resume`, and `inspect`. |
| `Review` | `ScoredChunk::chunk_index` would communicate intent better as `u32`. |

### rag-chunking

Clean overall. No blocking issues were confirmed in either pass.

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `chunking_semantic.rs:32` | The `#[allow(clippy::disallowed_methods)]` attribute appears attached to the wrong function. |
| `Verified` | `chunking_token.rs:39` | Uses underscore-prefixed `bpe._decode_native_and_split(...)` without documenting why that dependency is acceptable. |
| `Verified` | `chunking_paragraphs.rs:33-61` | `overlap_ratio` is accepted by the paragraph chunker but not applied between normal packed paragraph chunks. |
| `Verified` | `rustfmt.toml:1` | Formatter edition drift affects this crate too. |

#### Suggestions

| Tag | Finding |
|-----|---------|
| `Review` | `chunking_chars.rs` overlap can start mid-word. |
| `Review` | Missing empty-input tests for some strategies. |
| `Review` | `chunking_recursive.rs` deserves a comment on the `approx_chars / 4` fallback. |
| `Review` | `settings.rs` `OnceLock` caching complicates hot reload in tests. |
| `Review` | `ChunkingStrategy` could derive `Serialize` and `Deserialize`. |
| `Review` | `semantic_globally_disabled` could use `OnceLock`. |
| `Review` | `parse_heading` conflates malformed and absent headings. |

### rag-client

#### Blocking

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `lib.rs:1` | Missing `#![deny(unsafe_code)]`. |
| `Verified` | `client.rs:71,90` | `response.text().await.unwrap_or_default()` swallows body-read failures and treats them as empty bodies. |

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `types.rs:11,36,58,86` | Request types lack `Deserialize`, limiting testability and reuse. |
| `Review` | `types.rs:16` | `IngestRequest.dry_run` lacks `#[serde(default)]`, which becomes awkward if `Deserialize` is later added. |
| `Review` | `trait_def.rs:11-25` | `ApiClient` trait methods are undocumented. |
| `Review` | `tenant_id.rs` | Default tenant `"default"` is not exposed as a shared constant. |

#### Suggestions

| Tag | Finding |
|-----|---------|
| `Review` | Consider `impl Default for TenantId`. |
| `Review` | `collection_stats` percent-encoding is aggressive for path segments. |
| `Review` | Request structs may benefit from builders or `Default` derives. |

### rag-core

#### Blocking

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `lib.rs` | Missing `#![deny(unsafe_code)]`. |

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `ingest.rs:297-298` | No traversal restriction on ingestion paths before file reads. |
| `Review` | `retrieval.rs:179` | `unwrap_or(0)` on malformed or missing `chunk_index` payload risks silent corruption. |
| `Review` | `ingest.rs:380` | Token count approximation is knowingly coarse and should be documented. |
| `Verified` | `config.rs:269` | `bootstrap_platform_api_key` still uses `Option<String>` instead of `SecretString`. |
| `Verified` | `config.rs:227` | `AppConfig` derives `Debug`, increasing the risk of plaintext secret exposure. |
| `Review` | `migrations/0003_auth.sql:3` | Migration lacks `IF NOT EXISTS`, unlike earlier ones. |
| `Review` | `ingest.rs:713` | `WalkDir::new(dir)` should explicitly disable symlink following. |

#### Positive Highlights

- SQL access is consistently parameterized through `sqlx`
- tenant filters appear consistently in SQL and Qdrant paths
- retry logic exists for embedder and LLM backends
- OCR subprocess handling follows the documented project pattern
- transactions and row-level locking are used appropriately

### rag-server

This crate remains the highest-risk area in the workspace.

#### Blocking

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `lib.rs:1` | Missing `#![deny(unsafe_code)]`. |
| `Verified` | `state.rs:90-94` | `ApiError` converts `anyhow::Error` into the full formatted error chain and returns it to clients. |
| `Verified` | `routes/api_keys.rs:62,115,146,186` | Several handlers embed raw backend error strings directly in 500 responses. |
| `Verified` | `middleware/authz.rs:57` | Unrecognized routes fall through to `None`, so authorization is fail-open rather than fail-closed. |

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `routes/search.rs:63,99` | `top_k` is effectively unbounded. |
| `Review` | `routes/search.rs:33`, `routes/chat.rs:58` | Query strings have no explicit size bound. |
| `Review` | `routes/api_keys.rs:41-68` | `service_account.name` lacks length and character validation. |
| `Verified` | `routes/ingest.rs:43-113` | `ingest_paths` accepts arbitrary server-side paths and does not enforce a safe base directory. |
| `Review` | `middleware/rate_limit.rs:71-76` | `tenant_buckets` never evicts old entries. |
| `Review` | `middleware/auth.rs:143` | `Bearer` parsing is case-sensitive. |
| `Review` | `routes/chat.rs:126-148` | Error classification depends on substring matching of error messages. |
| `Review` | `routes/health.rs:42,52` | Readiness responses expose raw backend error detail. |

#### Positive Highlights

- middleware layering order looks correct
- API key verification uses constant-time comparison
- OIDC validation restricts algorithms and checks `kid`, issuer, and audience
- tenant binding for service-account keys prevents cross-tenant abuse
- RBAC is capability-based

### rag-cli

No blocking issues were confirmed in either pass.

#### Warnings

| Tag | File | Finding |
|-----|------|---------|
| `Review` | `rustfmt.toml:1` | Formatter edition mismatch. |
| `Review` | `commands/api_key.rs` | No direct test coverage for `api_key generate`, despite non-trivial output logic. |
| `Review` | `commands/chat.rs:51` | Unnecessary `.clone()` on owned `collection`. |
| `Review` | `commands/api_key.rs:37-51` | Does not use the shared `print_or_json` helper. |

#### Suggestions

| Tag | Finding |
|-----|---------|
| `Review` | `FakeClient` test boilerplate is duplicated across modules. |
| `Review` | `chat --query` and `search --query` may be more ergonomic as positional args. |
| `Review` | One e2e assertion is materially looser than related unit assertions. |
| `Review` | `SearchMode` variants need doc comments for `--help`. |

## Recommended Fix Order

1. Fix `rag-server` authz to fail closed for unmapped routes.
2. Stop returning raw internal error strings from `rag-server` responses.
3. Restrict or redesign path-based ingestion in `rag-server` and `rag-core`.
4. Add `#![deny(unsafe_code)]` consistently to the remaining library crates.
5. Convert remaining `rag-core` secrets to `SecretString` and avoid `Debug` exposure.
6. Resolve the `agent-core` schema-vs-serde contract mismatch.
7. Clean up the lower-risk client, formatter, and chunking issues.

## Notes

This report is the single source of truth going forward. It preserves the broader workspace coverage from the original review while making confidence explicit based on the manual re-verification pass.
