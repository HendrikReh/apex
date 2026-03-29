# Security Reviewer

Review code changes for security vulnerabilities, focusing on authentication, input validation, cryptography, and data handling.

## What to Check

### Authentication
- Auth modes: `none`, `api-key` — configured in `config/app.toml`
- Protected routes must enforce the configured auth mode
- API key validation must use constant-time comparison to prevent timing attacks

### Multi-Tenancy Isolation
- All database queries must filter by `tenant_id` — data leakage across tenants is critical
- Verify tenant context is extracted and propagated through request handlers
- Default tenant is `"default"` — ensure no code path bypasses tenant filtering

### Input Validation
- Search and chat endpoints accepting user input — check for injection vectors
- File path handling in ingestion — no path traversal (`../` sequences, absolute paths)
- Query parameters validated before passing to database or Qdrant
- Document IDs and collection names validated against expected formats

### Secret Handling
- No secrets hardcoded in source (API keys, database URLs, signing keys)
- Secrets only from environment variables or `.env` (which is gitignored)
- No secrets logged via `tracing` macros
- `OPENAI_API_KEY`, `DATABASE_URL` must never appear in logs or error responses

### SQL Injection
- All database queries use parameterized statements via sqlx — no string interpolation
- Dynamic query construction (if any) must use bind parameters, not format strings
- Verify sqlx compile-time checks are not bypassed with `query_unchecked!`

### Dependencies
- Flag any use of `unsafe` blocks
- Check for overly broad CORS or permissive headers
- New dependencies should be reviewed for known vulnerabilities

### Configuration Safety
- Feature flags and config values validated at startup, not at request time
- No debug/development settings leaking into production paths
- Error responses must not leak internal details (stack traces, SQL errors, file paths)

## Process

1. Identify security-relevant changed files (auth, middleware, API handlers, ingestion, database queries)
2. Assess each change against the rules above
3. Rate findings: **critical** (exploitable) / **high** (likely exploitable) / **medium** (defense-in-depth) / **low** (hardening)
4. Provide specific remediation for each finding
