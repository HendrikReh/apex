# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

[GitHub Repository](https://github.com/HendrikReh/apex)

## Mandatory: Beads Issue Gate

**HARD GATE — Do NOT write any code until a beads issue exists for the work.**

Before editing any `.rs`, `.toml`, `.sql`, `.yaml`, or `.yml` file:
1. Run `bd create --title="..." --type=<type> --priority=<N>` to create an issue
2. Run `bd update <id> --claim` to mark it in-progress
3. Only then begin implementation

If the user says "skip beads", run `touch /tmp/apex-beads-gate` and proceed without an issue.

This is enforced by a PreToolUse hook — edits to code files will be **blocked** without an active gate.

## Project Overview

Apex is a clean-room rebuild of [projectAlpha](https://github.com/HendrikReh/projectAlpha) — a Rust workspace for RAG (Retrieval-Augmented Generation). It ingests documents (PDF, Markdown, plain text at MVP), chunks with multiple strategies, generates embeddings (OpenAI or mock), and stores in Postgres (metadata) + Qdrant (vectors) for hybrid retrieval (dense + BM25 sparse).

## Reference Implementation

The source of truth for behavior is **projectAlpha** at `/Users/hendrik/Developer/projectAlpha`. When implementing features, read the corresponding projectAlpha source to understand existing behavior, then reimplement cleanly — do not copy-paste.

Key reference paths:
- Crate source: `/Users/hendrik/Developer/projectAlpha/crates/`
- Migrations: `/Users/hendrik/Developer/projectAlpha/crates/rag-core/migrations/`
- Config: `/Users/hendrik/Developer/projectAlpha/config/app.toml`
- OpenAPI spec: `/Users/hendrik/Developer/projectAlpha/docs/openapi.yaml`
- Agent specs: `/Users/hendrik/Developer/projectAlpha/config/agents/`
- ADRs: `/Users/hendrik/Developer/projectAlpha/docs/architect/adr/`

Rebuild plan: `docs/plans/rebuild-plan.md`

## Commands

### Prerequisites

Required tools: Rust (edition 2024), [just](https://github.com/casey/just) task runner, Docker (for Postgres + Qdrant).

### Build & Test

```bash
cargo check                # Quick compile check (use before commits)
just test                  # Full suite: fmt check -> clippy -> cargo test
just fmt                   # Check formatting (no changes)
just clippy                # Clippy with strict settings
cargo test -p <crate> <test_name>              # Single test by name
cargo test -p <crate> <test_name> -- --nocapture  # With stdout visible
```

### Run Services

```bash
just up                    # Start Postgres + Qdrant (docker compose)
just run-server            # Server with OpenAI embeddings (needs OPENAI_API_KEY)
just run-server-mock       # Server with mock embeddings (no API key)
```

## Architecture

### Crate Dependency Flow

```text
rag-cli -> rag-client -> reqwest (HTTP client for server API)
rag-server -> rag-core -> rag-chunking
           -> agent-core (graph-flow agent orchestration, post-MVP)
           -> rag-evidence (post-MVP)
           -> rag-notifications (post-MVP)
           -> rag-sbom (deferred hardening)
```

**Library crates** (`rag-core`, `rag-chunking`, `agent-core`, `rag-client`, `rag-evidence`, `rag-license`, `rag-notifications`, `rag-sbom`): Must NOT write to stdout/stderr or call `std::process::exit`. All errors as `Result` types.

**Binary crates** (`rag-server`, `rag-cli`): User-facing, may write to console.

### Crate Boundary Rules

| Crate | May depend on | May NOT depend on |
|---|---|---|
| `rag-chunking` | None (pure logic + tiktoken-rs) | Any workspace crate |
| `rag-core` | `rag-chunking` | `rag-server`, `rag-cli`, `agent-core` |
| `agent-core` | `rag-core` | `rag-server`, `rag-cli` |
| `rag-server` | `rag-core`, `agent-core`, `rag-evidence`, `rag-notifications` | `rag-cli` |
| `rag-client` | None (reqwest only) | Any workspace crate |
| `rag-cli` | `rag-client`, `rag-evidence` | `rag-core`, `rag-server` |
| `obfuscate-macros` | None (proc-macro) | Any workspace crate |
| `test-support` | `axum`, `tokio` | Any workspace crate |

### Key Design Decisions (carried from projectAlpha)

- **ADR-001**: Never use `.unwrap()` or `.expect()` in production code (enforced by `clippy.toml`)
- **ADR-002**: Document identity is `(tenant, document_id)`. Collection is routing metadata, not identity.
- **Multi-tenancy**: Day 1 requirement. All queries filter by `TenantId`. Default tenant: `"default"`.

## Code Conventions

### Error Handling (ADR-001, enforced by clippy)

- **NEVER** use `.unwrap()` or `.expect()` in production code — `clippy.toml` disallows them
- `.expect_err()` is NOT disallowed — only `.expect()` and `.unwrap()` on `Result`/`Option`
- Propagate with `?` and add context with `anyhow::Context`
- Functions returning `Result` already enforce `#[must_use]` — do NOT add the annotation

### Clippy False Positives

`tracing` macros and `serde_json::json!` internally use `.expect()`. Add `#[allow(clippy::disallowed_methods)]` with a comment to affected functions. Avoid module-level `#![allow(...)]`.

### Formatting

`rustfmt.toml`: `max_width = 100`, `edition = "2024"`, Unix newlines, `reorder_imports = true`.

### Configuration

Two-tier system:

1. **`config/app.toml`**: Non-secret settings (feature flags, chunking params, endpoints)
2. **`.env`**: Secrets only (`OPENAI_API_KEY`, `DATABASE_URL`, etc.)

Precedence: Environment variables > `config/app.toml` > hardcoded defaults.

### Test Patterns

- Use `test-support::spawn_app(Router)` for ephemeral Axum test servers
- Axum 0.7 routes use `:param` syntax, NOT `{param}`
- Use mock embedder for all automated tests (no API key required)

## Code Patterns & Pitfalls

Lessons learned from projectAlpha — avoid these in the rebuild:

- **Subprocess timeout**: `tokio::time::timeout` does NOT kill child processes — use `try_wait()` polling with `child.kill()`
- **regex_lite backreferences**: `\1` compiles but silently never matches — use separate patterns per tag in a loop
- **mail-parser body_text()**: Auto-generates text for HTML-only emails keeping `<script>`/`<style>` — check for real `text/plain` part first

## Task Tracking (Beads)

See **Mandatory: Beads Issue Gate** above. Full beads workflow and worktree instructions in `AGENTS.md`.

## Verification & Testing Policies

- **Before any commit**: Run `cargo fmt --all` then `cargo check`. This applies to direct work AND subagent-dispatched work — include both commands in every implementer subagent prompt.
- **NEVER run `just test` autonomously.** Only run it when the user explicitly requests it.
- **Doctest compilation requires Postgres running** (sqlx macros verify SQL at compile time via `DATABASE_URL`). Run `just up` before `just test`.

## sqlx Migrations

sqlx macros verify SQL at compile time. **NEVER run migration SQL files with raw `psql`** — this bypasses `_sqlx_migrations` tracking and causes "relation already exists" errors.

## Project Policies

- No backward compatibility required — code, data schema, API contracts, or persisted data (`data/docker/`). Delete unused code, don't add shims or migration paths. Reinitialize services from scratch when needed (`just down-v && just up`)
- Do what has been asked; nothing more, nothing less
- NEVER create files unless absolutely necessary; prefer editing existing files
- NEVER proactively create documentation files (*.md) unless explicitly requested

---

**Version**: 0.10.1
**Last updated**: 2026-04-01
**Maintained By**: <hendrik.reh@blacksmith-consulting.ai>
