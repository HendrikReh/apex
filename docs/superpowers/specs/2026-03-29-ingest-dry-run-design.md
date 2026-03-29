# Ingest `--dry-run` Flag — Design Spec

**Beads issue:** apex-65e
**Date:** 2026-03-29
**Status:** Approved

---

## Goal

Add a `--dry-run` flag to the CLI ingest command that shows what *would* be ingested without writing anything to Postgres or Qdrant. Validates paths, extracts text, computes checksums, and checks for unchanged files — but skips chunking, embedding, and all persistence.

## Architecture

### Component: CLI (`crates/rag-cli/src/commands/ingest.rs`)

Add `--dry-run` flag to the `Ingest` command struct. Pass it through to the client's `IngestRequest`. When the response returns, prefix output with `[dry-run]`.

### Component: HTTP Client (`crates/rag-client`)

Add `dry_run: bool` field to `IngestRequest`. Defaults to `false`.

### Component: Server Route (`crates/rag-server/src/routes/ingest.rs`)

Add `dry_run` field to `IngestPathsRequest` (defaults to `false` via serde). Pass it into `IngestFileRequest` and `IngestDirectoryRequest` when constructing them.

Not in scope: `/ingest/upload` (multipart endpoint). Only `/ingest` (path-based).

### Component: Core IngestService (`crates/rag-core/src/ingest.rs`)

**New field on both request types:**

- `IngestFileRequest` (line ~41): add `pub dry_run: bool`
- `IngestDirectoryRequest` (line ~58): add `pub dry_run: bool`

`ingest_directory()` propagates `dry_run` into each child `IngestFileRequest`.

**`ingest_file()` behavior when `dry_run` is true:**

1. Load sidecar — same as normal.
2. Extract text + compute checksum — same as normal.
3. Check Postgres for existing checksum — same as normal (read-only query to determine new vs skipped).
4. **If checksum matches (unchanged file):** return `IngestOutcome { skipped: true, chunks_created: 0, .. }` immediately. Do NOT call `upsert_document()` — the normal skip path updates metadata on unchanged docs, but dry-run must bypass that write entirely.
5. **If checksum differs or file is new:** return `IngestOutcome { skipped: false, chunks_created: 0, .. }` immediately. No chunking, no embedding, no persistence.

### Data Flow

```
CLI --dry-run ./docs --collection demo
  → IngestRequest { paths, collection, dry_run: true }
  → POST /ingest
  → Server: canonicalize paths, set dry_run on each request
  → IngestService::ingest_file():
      extract → checksum → check Postgres (read-only) → STOP
  → IngestResponse { documents: N, chunks: 0, skipped: M, failures: [...] }
  → CLI: "[dry-run] Ingested 5 documents, 0 chunks, 2 skipped"
```

## Response

Same `IngestResponse` shape as normal ingest:

| Field | Dry-run meaning |
|-------|----------------|
| `documents` | Files successfully examined (extraction + checksum completed without error) |
| `chunks` | Always 0 (chunking not performed) |
| `skipped` | Files with unchanged checksums |
| `failures` | Extraction or path resolution errors |

## Error Handling

Identical to normal ingest — extraction failures, path resolution errors, and unsupported file types are reported in the `failures` array. The only difference is that no writes occur.

## Testing

Integration tests (require running Postgres + Qdrant, since `IngestService` uses concrete stores):
- Verify `ingest_file()` with `dry_run: true` does not write to Postgres or Qdrant.
- Verify unchanged file returns `skipped: true, chunks_created: 0`.
- Verify new file returns `skipped: false, chunks_created: 0`.

CLI test:
- Verify `--dry-run` flag is accepted and output includes `[dry-run]` prefix.

## Non-Goals

- Dry-run for `/ingest/upload` (multipart)
- Reporting chunk counts in dry-run (would require running the chunking pipeline)
- Any new CLI output format beyond the `[dry-run]` prefix
