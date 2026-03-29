---
name: diagnose-sqlx
description: Debug sqlx compile-time check failures — verifies Postgres connectivity, migration state, and _sqlx_migrations consistency
---

# Diagnose sqlx

Systematically debug sqlx compile-time failures in the rag-core crate.

## Background

sqlx macros (`query!`, `query_as!`, `query_scalar!`) verify SQL against a live Postgres instance
at compile time via `DATABASE_URL`. When `cargo check` fails with sqlx errors, the cause is
usually one of: Postgres not running, missing migration, or `_sqlx_migrations` table out of sync.

## Diagnostic Steps

Run these in order. Stop at the first failure and fix it.

### Step 1: Verify Docker Services

```bash
docker compose -f docker-compose.yml ps
```

Postgres must be running. If not: `just up`

### Step 2: Verify DATABASE_URL Connectivity

```bash
psql "$DATABASE_URL" -c "SELECT 1;"
```

If this fails, check `.env` for correct `DATABASE_URL` (default: `postgres://postgres:postgres@127.0.0.1:5432/postgres`).

### Step 3: Compare Migration Files vs Applied Migrations

```bash
# Files on disk
ls crates/rag-core/migrations/ | sort

# Applied in database
psql "$DATABASE_URL" -c "SELECT version, description, installed_on FROM _sqlx_migrations ORDER BY version;"
```

Compare the two lists:
- **File exists but not in DB**: Migration needs to be applied (Step 4)
- **In DB but file missing**: Orphaned migration record — investigate
- **Both match**: Schema should be current — proceed to Step 5

### Step 4: Apply Pending Migrations

**Preferred**: Use sqlx CLI:
```bash
sqlx migrate run --source crates/rag-core/migrations
```

**Alternative**: Start the server once (it auto-migrates on startup):
```bash
EMBEDDER=mock RUST_LOG=info cargo run -p rag-server
```
Then Ctrl+C after you see migration output.

**NEVER use raw `psql` to run migration files.** This creates schema objects but bypasses
the `_sqlx_migrations` tracking table, causing "relation already exists" on next server start.

### Step 5: Verify Compile-Time Checks Pass

```bash
cargo check -p rag-core
```

### Step 6: If "relation already exists" Error

This means someone ran a migration via `psql` instead of sqlx. Fix:

```bash
# Identify the conflicting migration number (e.g., 0014)
# Drop the manually-created objects
psql "$DATABASE_URL" -c "DROP TABLE IF EXISTS <table_name> CASCADE;"

# Re-apply through sqlx
sqlx migrate run --source crates/rag-core/migrations
```

### Step 7: If Column/Type Mismatch

The migration was applied but the schema doesn't match what sqlx macros expect:

```bash
# Check actual schema
psql "$DATABASE_URL" -c "\d <table_name>"

# Compare with the query in the Rust source
# Look for column name typos, type mismatches, or missing columns
```

## Common Error Patterns

| Error Message | Likely Cause | Fix |
|---|---|---|
| `error returned from database: relation "X" does not exist` | Migration not applied | Step 4 |
| `error returned from database: relation "X" already exists` | Migration ran via psql | Step 6 |
| `column "X" of relation "Y" does not exist` | Migration partially applied or wrong migration | Step 7 |
| `could not determine data type of parameter` | Type mismatch in query macro | Check query param types vs schema |
| `error communicating with database` | Postgres not running | Step 1 |
| `password authentication failed` | Wrong DATABASE_URL | Step 2 |

## Prevention

Use the `/create-migration` skill for new migrations — it enforces the correct workflow.
