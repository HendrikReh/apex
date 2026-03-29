---
name: review
description: Use when reviewing code changes before commit or PR — dispatches architecture, code, migration, and security review agents, checks project-specific conventions. Also use when the user says "review my changes", "check this code", or "is this ready to commit".
---

# Code & Security Review

Review current changes against project conventions and security rules. This skill dispatches specialized review agents rather than doing a surface-level scan — the agents know the project's specific rules (ADR-001, crate boundaries, multi-tenancy).

## Step 1 — Identify scope

```bash
git diff --name-only HEAD
git diff --name-only --cached
git ls-files --others --exclude-standard
```

If no changes and no untracked files -> stop and report "nothing to review."

Categorize changed files to determine review depth:

| Category | Files | Review Type |
|----------|-------|-------------|
| **Server/core** | `crates/rag-server/`, `crates/rag-core/`, `crates/agent-core/` | Full code + security review |
| **CLI/client** | `crates/rag-cli/`, `crates/rag-client/` | Code review + crate boundary check |
| **Migrations** | `crates/rag-core/migrations/` | Migration review |
| **Config/docs** | `config/`, `docs/`, `.claude/` | Doc consistency check only |

Server and core changes get the security review because they handle auth and user input directly. CLI/client changes skip security review since they're HTTP clients, not servers.

## Step 2 — Dispatch architecture checker agent

Run the architecture checker agent at `.claude/agents/architecture-checker.md`. It checks:
- Crate boundary rules (no stdout/stderr/`process::exit` in libraries)
- Dependency direction (libraries must not depend on binaries)
- Workspace dependency usage (`{ workspace = true }`)
- Multi-tenancy: all queries filter by `TenantId`
- ADR-002: document identity is `(tenant, document_id)`

## Step 3 — Dispatch code review agent

Run the code review agent at `.claude/agents/code-reviewer.md`. It checks:
- ADR-001 error handling (no `.unwrap()`/`.expect()` in lib crates)
- Axum 0.7 conventions (`:param` syntax, proper state extraction)
- Serde derive conventions
- Formatting compliance (`max_width = 100`)
- Test pattern correctness

## Step 4 — Dispatch migration review agent

For changes in `crates/rag-core/migrations/`, run the migration review agent at `.claude/agents/migration-reviewer.md`. It checks:
- Sequential numbering (no gaps, no duplicates)
- Idempotency guards (`IF NOT EXISTS`, `IF EXISTS`)
- Rollback SQL included as comments for destructive changes
- Applied via sqlx, never raw psql

## Step 5 — Dispatch security review agent

For server/core changes, run the security review agent at `.claude/agents/security-reviewer.md`. It checks:
- Auth mode handling (none, api-key)
- Secret handling (no secrets in code, proper `.env` usage)
- Input validation at system boundaries
- SQL injection via raw queries (prefer sqlx parameterized)
- Path traversal in file ingestion
- Unsafe blocks (denied by `#![deny(unsafe_code)]`)
- Multi-tenancy enforcement (tenant isolation in all queries)

## Step 6 — Classify findings

| Severity | Definition | Examples |
|----------|-----------|----------|
| **Blocking** | Must fix before merge | Security issue, `.unwrap()` in lib crate, crate boundary violation, missing tenant filter |
| **Warning** | Should fix | Missing test coverage, weak validation, inconsistent error messages, missing `anyhow::Context` |
| **Suggestion** | Nice to have | Naming improvements, doc additions, minor performance |

## Step 7 — Report and iterate

Summarize findings by severity. Present blocking issues first. Ask the user which non-blocking issues to address.

If the user wants to fix issues:
1. Fix blocking issues first
2. Re-run `cargo check -p <affected-crate>` after each fix
3. After all fixes, re-run a targeted review on just the changed files to confirm

## Cross-references

- `/pr-ready` — full PR workflow after review passes
- `/resolve-review-comments` — addressing review feedback from bots
