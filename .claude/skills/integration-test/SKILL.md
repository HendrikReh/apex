---
name: integration-test
description: Use when running integration tests — ensures Docker services are up, env vars set, and the correct test suite is selected. Also use when the user says "run integration tests", "test against real DB", "test with Qdrant", or asks about which integration suite to use for their change.
---

# Integration Test Runner

Run integration tests with the right services, env vars, and suite selection. This skill exists because integration tests silently skip (returning green) when env vars are missing — a common source of false confidence.

## Step 1: Check Docker Services

```bash
docker compose ps --format "table {{.Name}}\t{{.Status}}\t{{.Ports}}"
```

Required services depend on the suite (see table below). If services aren't running, tell the user to run `just up` and stop — don't proceed with tests that will silently skip.

## Step 2: Select the Right Suite

Choose based on what the user changed or wants to test:

| Suite | Command | Env Var | Services Required | When to Use |
|-------|---------|---------|-------------------|-------------|
| **stores** | `just integration-stores` | `RUN_QDRANT_INTEGRATION_TESTS=1` | Postgres + Qdrant | Changed storage layer, embeddings, collections, or Qdrant interactions |
| **api** | `just integration-api` | `RUN_QDRANT_INTEGRATION_TESTS=1` + `INTEGRATION_API_KEY` | Postgres + Qdrant + running server | Changed API handlers, routes, request/response formats |
| **auth** | `just integration-auth` | `RUN_AUTH_INTEGRATION_TESTS=1` | Running server in API-key mode | Changed auth middleware |
| **all** | `just integration-tests` | all of the above | all of the above | Full verification before merge |

**Server startup for api/auth suites:**
- API suite: `just run-server-mock` (or `just run-server` with `OPENAI_API_KEY`)
- Auth suite: `AUTH_MODE=api-key AUTH_API_KEYS=test-key-1,test-key-2 cargo run -p rag-server`

## Step 3: Run the Suite

Set the env vars and run. The tests use `#[ignore]` and are gated by env var checks — without the env var, tests return early with a skip message rather than failing, which means a missing env var looks like a pass.

Example for stores:
```bash
RUN_QDRANT_INTEGRATION_TESTS=1 cargo test --test integration_stores -p rag-core -- --ignored
```

## Step 4: Interpret Results

**On success:** Report passing test count and suite name.

**On failure:** Show:
1. The specific test name (e.g., `test_ensure_collection_validates_schema`)
2. The assertion message and expected vs actual values
3. Relevant backtrace lines (filter to `rag_` and `integration_` frames)

**Common failure patterns:**

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| "connection refused" on port 6334 | Qdrant not running | `just up` |
| "connection refused" on port 5432 | Postgres not running | `just up` |
| All tests "skipped" (0 failures, 0 passes) | Env var not set | Set `RUN_QDRANT_INTEGRATION_TESTS=1` |
| "Server not responding" | Server not started for API/auth suite | Start server (see Step 2) |
| Sporadic failures on collection ops | Concurrent test runs | Ensure `#[serial]` attribute; don't run integration tests across worktrees simultaneously |
| Schema/migration errors | Missing migrations | `sqlx migrate run` or see `/diagnose-sqlx` |

## Important Constraints

- **Never run integration tests in parallel across worktrees** — Docker services (Postgres, Qdrant) are shared and will conflict.
- Tests use `#[serial]` to prevent concurrent DB access within a suite.
- The `common::test_config()` helper reads `DATABASE_URL` and `QDRANT_URL` from env with sensible localhost defaults.

## Cross-references

- `/debug` — for diagnosing test failures
- `/diagnose-sqlx` — for migration/schema issues
- `/write-test` — for writing new integration tests
