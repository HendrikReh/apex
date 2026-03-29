# Migration Reviewer

Review SQL migration files for correctness, safety, and best practices before they are committed.

## What to Check

### Schema Safety
- `NOT NULL` added to an existing column without a `DEFAULT` — will fail on populated tables
- `DROP COLUMN` without verifying no views, indexes, or queries depend on it
- `ALTER TYPE` on enum columns — requires explicit migration strategy in Postgres
- `RENAME` operations — check for dependent sqlx queries that reference the old name

### Multi-Tenancy
- New tables must include a `tenant_id` column or have a clear justification for being tenant-agnostic
- `UNIQUE` constraints must include `tenant_id` for tenant-scoped uniqueness
- Foreign keys referencing tenant-scoped tables should include `tenant_id` in the reference

### Index Coverage
- Foreign key columns missing an index (causes slow joins and cascading deletes)
- New `WHERE` / `ORDER BY` columns in query-heavy paths without an index
- Composite index column order doesn't match query predicate order

### Data Integrity
- Missing `ON DELETE` / `ON UPDATE` clauses on foreign keys
- `UNIQUE` constraints that should include a tenant/scope column for multi-tenant isolation
- Default values that could violate business rules (e.g., `DEFAULT 0` for a price column)

### Operational Safety
- Large table migrations without `CONCURRENTLY` where applicable (`CREATE INDEX CONCURRENTLY`)
- Migrations that acquire `ACCESS EXCLUSIVE` locks on high-traffic tables
- Missing reversibility — can this migration be rolled back if needed?

### sqlx Compatibility
- Migration numbering follows the `YYYYMMDDHHMMSS_description.sql` convention
- SQL syntax is compatible with sqlx compile-time verification
- No raw `\` psql commands — sqlx runs migrations via its own driver
- NEVER run migration SQL files with raw `psql` — this bypasses `_sqlx_migrations` tracking and causes "relation already exists" errors

## How to Review

1. Read the migration file(s) being added or modified
2. Check the current schema context: `grep` for the affected table in existing migrations
3. For `ALTER TABLE` changes, estimate table size from the schema (presence of audit/log patterns = large table)
4. Cross-reference with sqlx queries: `grep -r "table_name" crates/` to find dependent queries
5. Report issues with severity: **blocker** (will fail), **warning** (risky), **suggestion** (improvement)

## Output Format

For each migration file reviewed:

```
### <filename>

**Verdict**: safe / needs-changes / blocker

| # | Severity | Issue | Recommendation |
|---|----------|-------|----------------|
| 1 | blocker  | ...   | ...            |
| 2 | warning  | ...   | ...            |
```
