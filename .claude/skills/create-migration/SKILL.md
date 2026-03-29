---
name: create-migration
description: Create a new sqlx migration with correct numbering and apply it through the proper sqlx workflow — never raw psql
disable-model-invocation: true
---

# Create Migration

Scaffold and apply a new sqlx migration for the rag-core Postgres schema.

## Why This Skill Exists

sqlx macros (`query!`, `query_as!`) verify SQL against a live Postgres instance at compile time.
Migrations MUST be applied through sqlx's migration runner (not raw `psql`) so that the
`_sqlx_migrations` tracking table stays in sync. Violating this causes "relation already exists"
errors on server startup.

## Workflow

### 1. Determine Next Migration Number

```bash
ls crates/rag-core/migrations/ | sort | tail -1
```

Increment the numeric prefix (e.g., if last is `0042_foo.sql`, next is `0043`).

### 2. Create the Migration File

Write the new file at `crates/rag-core/migrations/<NNNN>_<descriptive_name>.sql`.

Rules:
- Use `IF NOT EXISTS` / `IF EXISTS` guards where appropriate for idempotency
- Include a brief comment at the top explaining the purpose
- For destructive changes, add the rollback SQL as a comment block

### 3. Apply via sqlx (NEVER raw psql)

```bash
sqlx migrate run --source crates/rag-core/migrations
```

If `sqlx` CLI is not available, start the server once to auto-apply:
```bash
just run-server-mock
```
Then Ctrl+C after migrations complete.

### 4. Verify Compile-Time Checks

```bash
cargo check -p rag-core
```

This confirms sqlx macros can see the new schema objects.

### 5. Recovery: If Migration Was Applied Manually

If someone already ran the SQL via `psql` and the server fails with "relation already exists":

1. Drop the manually-created objects:
   ```sql
   DROP TABLE IF EXISTS <table_name>;
   ```
2. Re-apply through sqlx (step 3 above)

## Checklist

- [ ] Migration number is sequential (no gaps, no duplicates)
- [ ] File is in `crates/rag-core/migrations/`
- [ ] Applied via `sqlx migrate run`, NOT `psql`
- [ ] `cargo check -p rag-core` passes
- [ ] If adding columns to existing tables, consider `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`
