---
name: debug
description: Use when tests fail, builds break, or runtime errors occur — classifies failures and provides targeted resolution steps for this Rust workspace. Also use when encountering panics, timeouts, connection errors, or unexpected behavior during development.
---

# Debug Workflow

Systematic diagnosis when tests, builds, or runtime behavior fail.

## Step 1 — Classify the failure

| Symptom | Likely Cause | Go to |
|---------|-------------|-------|
| `error[E...]` compilation error | Code error or missing import | Step 2a |
| Test assertion failure | Logic bug | Step 2b |
| `Blocking waiting for file lock` | Target-dir contention (IDE + terminal) | Step 2c |
| `connection refused` / DB errors | Services not running | Step 2d |
| Flaky test (passes on re-run) | Known flaky or timing issue | Step 2e |
| `thread 'main' panicked` | Unwrap/expect on None/Err | Step 2f |
| Timeout / process hangs | Subprocess not killed or async deadlock | Step 2g |
| sqlx compile-time errors | Schema/migration drift | Step 2h |

## Step 2a — Compilation error

1. Read the full error message carefully
2. Run `git diff` to see recent changes that may have caused it
3. Fix the error
4. Run `cargo check -p <crate>` (fast feedback)

## Step 2b — Test failure

1. Run the specific test with output: `cargo test -p <crate> <test_name> -- --nocapture`
2. Read the assertion message and expected vs actual values
3. Check the test setup — is the mock handler returning what's expected?
4. For oneshot tests: verify the request URI, method, and headers match the route registration
5. Fix and re-run the single test before running the full suite

## Step 2c — Build lock contention

1. Check for stale processes: `ps aux | grep cargo` and `ps aux | grep rust-analyzer`
2. Kill stale cargo processes if identified: `kill <PID>` (never `kill -9` unless unresponsive)
3. If contention persists, use an isolated `CARGO_TARGET_DIR` to avoid conflicts with rust-analyzer

## Step 2d — Service connection errors

1. Check Docker: `docker compose ps`
2. If services not running: tell user to run `just up`
3. If services are up but failing: check logs with `docker compose logs --tail=20 <service>`
4. Common ports: Postgres=5432, Qdrant=6333/6334
5. For sqlx compile-time errors: see Step 2h

## Step 2e — Flaky test

If a test is suspected flaky:
1. Re-run 3 times to confirm flakiness: `cargo test -p <crate> <test_name> -- --nocapture`
2. Check for timing-sensitive assertions (sleep-based, clock-based)
3. Check for shared state leaking between tests (missing `#[serial]` attribute)
4. If confirmed flaky, report to user — do not try to fix unless asked

## Step 2f — Panic (unwrap/expect)

This project forbids `.unwrap()` and `.expect()` in production code (ADR-001, enforced by clippy.toml). If a panic occurs:

1. Read the backtrace: `RUST_BACKTRACE=1 cargo test -p <crate> <test_name>`
2. Find the source — is it in production code or test code?
3. **Production code**: Replace with `?` + `anyhow::Context`. Add `#[allow(clippy::disallowed_methods)]` only if it's a false positive (tracing macros, serde_json::json!)
4. **Test code**: `.unwrap()` is acceptable in tests (clippy.toml allows it via `#[cfg_attr(test, allow(clippy::disallowed_methods))]`)

## Step 2g — Timeout / process hang

This is a documented pitfall in this codebase. `tokio::time::timeout` does NOT kill child processes — it only drops the future, leaving orphaned processes.

1. Check if the hang involves a subprocess (e.g., external tools)
2. Use the `try_wait()` polling pattern with `child.kill()` for subprocess management
3. For async deadlocks: check for `.await` inside `.lock()` — holding a `Mutex` guard across await points causes deadlocks with tokio
4. For integration tests that hang: verify the server is actually running and responsive

## Step 2h — sqlx compile-time errors

sqlx macros verify SQL at compile time against a real database. Common issues:

1. **"relation does not exist"**: Migrations not applied — run `sqlx migrate run`
2. **"already exists"**: Migration was applied via raw `psql` — see `/diagnose-sqlx` skill
3. **Offline mode**: If `.sqlx/` cache is stale, run `cargo sqlx prepare --workspace`
4. **Column mismatch**: Schema changed but query not updated — check the SQL against the current schema

## Additional Tools

- `RUST_LOG=debug cargo run -p rag-server` — verbose tracing output for runtime debugging
- `RUST_BACKTRACE=full` — full backtraces for panics
- `cargo test -p <crate> -- --list` — list all tests in a crate without running them

## Cross-references

- `/diagnose-sqlx` — deep-dive into sqlx compile-time failures
- `/integration-test` — for service-dependent test failures
- `/write-test` — for fixing or rewriting broken tests
