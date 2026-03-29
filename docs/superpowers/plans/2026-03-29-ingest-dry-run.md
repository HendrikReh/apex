# Ingest `--dry-run` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--dry-run` flag to the ingest pipeline that validates paths, extracts text, and checks checksums without writing to Postgres or Qdrant.

**Architecture:** Thread a `dry_run: bool` field from CLI -> client -> server -> core request types. In `IngestService::ingest_file()`, after extraction and read-only checksum lookup, return immediately when `dry_run` is true — skipping chunking, embedding, and all persistence (including the metadata update on unchanged files).

**Tech Stack:** Rust, clap (CLI), serde (JSON), axum (server), sqlx (Postgres)

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `crates/rag-core/src/ingest.rs` | Modify | Add `dry_run` to request types, implement early return |
| `crates/rag-core/tests/integration_ingest.rs` | Modify | Add dry-run integration tests |
| `crates/rag-server/src/routes/ingest.rs` | Modify | Accept + propagate `dry_run` from HTTP request |
| `crates/rag-client/src/types.rs` | Modify | Add `dry_run` to `IngestRequest` |
| `crates/rag-cli/src/cli.rs` | Modify | Add `--dry-run` flag to `Ingest` command |
| `crates/rag-cli/src/commands/ingest.rs` | Modify | Pass `dry_run`, prefix output with `[dry-run]` |
| `crates/rag-cli/src/main.rs` | Modify | Destructure and pass `dry_run` |
| `docs/openapi.yaml` | Modify | Add `dry_run` to `IngestPathsRequest` schema |

---

### Task 1: Core types — add `dry_run` field to request structs

**Files:**
- Modify: `crates/rag-core/src/ingest.rs:40-45,57-62,214-218`
- Modify: `crates/rag-server/src/routes/ingest.rs:59-63,79-83,169-173`
- Modify: `crates/rag-core/tests/integration_ingest.rs:65-69,89-93,118-122`

Adds the `dry_run: bool` field to both core request types and fixes all construction sites so the workspace compiles. No behavior change — all existing construction sites set `dry_run: false`.

- [ ] **Step 1: Add `dry_run: bool` to `IngestFileRequest`**

In `crates/rag-core/src/ingest.rs`, change the struct at line 41:

```rust
/// A request to ingest a single file.
#[derive(Debug)]
pub struct IngestFileRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
    pub dry_run: bool,
}
```

- [ ] **Step 2: Add `dry_run: bool` to `IngestDirectoryRequest`**

In `crates/rag-core/src/ingest.rs`, change the struct at line 58:

```rust
/// A request to ingest every supported file in a directory tree.
#[derive(Debug)]
pub struct IngestDirectoryRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
    pub dry_run: bool,
}
```

- [ ] **Step 3: Propagate `dry_run` in `ingest_directory()`**

In `crates/rag-core/src/ingest.rs`, update the child request construction at line 214:

```rust
let file_req = IngestFileRequest {
    path: pair.path.clone(),
    tenant: req.tenant.clone(),
    collection_override: req.collection_override.clone(),
    dry_run: req.dry_run,
};
```

- [ ] **Step 4: Fix server construction sites**

In `crates/rag-server/src/routes/ingest.rs`, add `dry_run: false` to all three construction sites:

Line 59 (`IngestDirectoryRequest` in `ingest_paths`):
```rust
let req = IngestDirectoryRequest {
    path: path.clone(),
    tenant: ctx.tenant.clone(),
    collection_override: payload.collection.clone(),
    dry_run: false,
};
```

Line 79 (`IngestFileRequest` in `ingest_paths`):
```rust
let req = IngestFileRequest {
    path: path.clone(),
    tenant: ctx.tenant.clone(),
    collection_override: payload.collection.clone(),
    dry_run: false,
};
```

Line 169 (`IngestFileRequest` in `ingest_upload`):
```rust
let req = IngestFileRequest {
    path: tmp.path().to_path_buf(),
    tenant: ctx.tenant,
    collection_override: collection,
    dry_run: false,
};
```

- [ ] **Step 5: Fix integration test construction sites**

In `crates/rag-core/tests/integration_ingest.rs`, add `dry_run: false` to all three construction sites:

Line 65 (`ingest_single_txt_file`):
```rust
.ingest_file(IngestFileRequest {
    path: dir.path().join("hello.txt"),
    tenant,
    collection_override: Some(format!("test_ingest_{suffix}")),
    dry_run: false,
})
```

Line 89 (`reingest_unchanged_file_is_skipped` closure):
```rust
let make_req = || IngestFileRequest {
    path: dir.path().join("stable.txt"),
    tenant: TenantId::new(&tenant_str).expect("tenant"),
    collection_override: Some(collection.clone()),
    dry_run: false,
};
```

Line 118 (`ingest_directory_processes_all_files`):
```rust
.ingest_directory(IngestDirectoryRequest {
    path: dir.path().to_owned(),
    tenant,
    collection_override: Some(format!("test_batch_{suffix}")),
    dry_run: false,
})
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check --workspace`
Expected: Success (no behavior change, all construction sites updated)

- [ ] **Step 7: Commit**

```bash
git add crates/rag-core/src/ingest.rs crates/rag-server/src/routes/ingest.rs crates/rag-core/tests/integration_ingest.rs
git commit -m "feat(ingest): add dry_run field to request types (no behavior change)"
```

---

### Task 2: Core logic — implement dry-run early return + integration tests

**Files:**
- Modify: `crates/rag-core/src/ingest.rs:131-171`
- Modify: `crates/rag-core/tests/integration_ingest.rs`

- [ ] **Step 1: Update `setup()` to return `Stores` for verification**

In `crates/rag-core/tests/integration_ingest.rs`, change the `setup()` function to return `Stores`:

```rust
async fn setup() -> Result<(IngestService, Stores, TempDir)> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    let stores = Stores::new(&config).await?;
    let service = IngestService::new(stores.clone(), &config)?;
    let dir = TempDir::new()?;
    Ok((service, stores, dir))
}
```

Update the three existing test destructurings to ignore the new return value:

```rust
// In ingest_single_txt_file:
let (service, _stores, dir) = setup().await.expect("setup");

// In reingest_unchanged_file_is_skipped:
let (service, _stores, dir) = setup().await.expect("setup");

// In ingest_directory_processes_all_files:
let (service, _stores, dir) = setup().await.expect("setup");
```

- [ ] **Step 2: Write the failing integration test — new file dry-run**

In `crates/rag-core/tests/integration_ingest.rs`, add after the existing tests:

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn dry_run_new_file_does_not_write() {
    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "dryrun.txt", "Content for dry-run test of a new file.");

    let tenant_str = format!("test-dry-new-{suffix}");
    let tenant = TenantId::new(&tenant_str).expect("tenant");
    let collection = format!("test_dry_new_{suffix}");

    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("dryrun.txt"),
            tenant: tenant.clone(),
            collection_override: Some(collection),
            dry_run: true,
        })
        .await
        .expect("dry-run ingest should succeed");

    assert!(!outcome.skipped, "new file should not be marked as skipped");
    assert_eq!(outcome.chunks_created, 0, "dry-run must not create chunks");

    // Verify nothing was persisted to Postgres.
    let checksum = stores
        .get_document_checksum(tenant.as_str(), &outcome.document_id)
        .await
        .expect("checksum query should succeed");
    assert!(checksum.is_none(), "dry-run must not write document to Postgres");

    // Verify nothing was written to Qdrant — the unique collection should not exist.
    let qdrant_exists = stores
        .collection_exists(&collection)
        .await
        .expect("collection_exists query should succeed");
    assert!(!qdrant_exists, "dry-run must not create Qdrant collection");
}
```

- [ ] **Step 3: Write the failing integration test — unchanged file dry-run**

In `crates/rag-core/tests/integration_ingest.rs`, add:

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn dry_run_unchanged_file_returns_skipped() {
    let (service, _stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(
        dir.path(),
        "existing.txt",
        "Content that will be ingested then dry-run checked.",
    );
    write_sidecar(dir.path(), "existing");

    let tenant_str = format!("test-dry-skip-{suffix}");
    let collection = format!("test_dry_skip_{suffix}");

    // First: real ingest to populate Postgres.
    let first = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("existing.txt"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection.clone()),
            dry_run: false,
        })
        .await
        .expect("initial ingest should succeed");
    assert!(!first.skipped);
    assert!(first.chunks_created > 0);

    // Second: dry-run on the same unchanged file.
    let second = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("existing.txt"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection),
            dry_run: true,
        })
        .await
        .expect("dry-run ingest should succeed");

    assert!(second.skipped, "unchanged file should be marked as skipped");
    assert_eq!(second.chunks_created, 0, "dry-run must not create chunks");
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p rag-core --test integration_ingest dry_run -- --ignored --nocapture`
Expected: Both tests FAIL — `dry_run_new_file_does_not_write` fails because `chunks_created > 0` (dry_run field is ignored), and `dry_run_unchanged_file_returns_skipped` fails because `chunks_created > 0`.

- [ ] **Step 5: Implement dry-run early return in `ingest_file()`**

In `crates/rag-core/src/ingest.rs`, insert the dry-run check after the checksum lookup (after line 140, before the existing `if existing_checksum.as_deref() == Some(...)` block at line 142):

```rust
// Dry-run: report what would happen without writing anything.
if req.dry_run {
    let skipped = existing_checksum.as_deref() == Some(&prepared.checksum);
    return Ok(IngestOutcome {
        document_id: prepared.document_id,
        collection: prepared.collection,
        chunks_created: 0,
        skipped,
    });
}
```

After the change, lines 136-152 of `ingest_file()` should read:

```rust
let existing_checksum = self
    .stores
    .get_document_checksum(req.tenant.as_str(), &prepared.document_id)
    .await
    .context("checking existing document checksum")?;

// Dry-run: report what would happen without writing anything.
if req.dry_run {
    let skipped = existing_checksum.as_deref() == Some(&prepared.checksum);
    return Ok(IngestOutcome {
        document_id: prepared.document_id,
        collection: prepared.collection,
        chunks_created: 0,
        skipped,
    });
}

if existing_checksum.as_deref() == Some(&prepared.checksum) {
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rag-core --test integration_ingest dry_run -- --ignored --nocapture`
Expected: Both `dry_run_new_file_does_not_write` and `dry_run_unchanged_file_returns_skipped` PASS.

- [ ] **Step 7: Run full workspace check**

Run: `cargo check --workspace`
Expected: Success

- [ ] **Step 8: Commit**

```bash
git add crates/rag-core/src/ingest.rs crates/rag-core/tests/integration_ingest.rs
git commit -m "feat(ingest): implement dry-run early return in ingest_file()"
```

---

### Task 3: Server + OpenAPI — accept `dry_run` in HTTP request

**Files:**
- Modify: `crates/rag-server/src/routes/ingest.rs:11-15,59-63,79-83`
- Modify: `docs/openapi.yaml:303-316`

- [ ] **Step 1: Add `dry_run` field to server's `IngestPathsRequest`**

In `crates/rag-server/src/routes/ingest.rs`, update the struct at line 11:

```rust
#[derive(Deserialize)]
pub struct IngestPathsRequest {
    pub paths: Vec<String>,
    pub collection: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}
```

- [ ] **Step 2: Use `payload.dry_run` in construction sites**

Replace the hardcoded `dry_run: false` (from Task 1) with `payload.dry_run` in both construction sites within `ingest_paths()`:

`IngestDirectoryRequest` (~line 59):
```rust
let req = IngestDirectoryRequest {
    path: path.clone(),
    tenant: ctx.tenant.clone(),
    collection_override: payload.collection.clone(),
    dry_run: payload.dry_run,
};
```

`IngestFileRequest` in `ingest_paths()` (~line 79):
```rust
let req = IngestFileRequest {
    path: path.clone(),
    tenant: ctx.tenant.clone(),
    collection_override: payload.collection.clone(),
    dry_run: payload.dry_run,
};
```

Note: `ingest_upload()` keeps `dry_run: false` — upload dry-run is explicitly out of scope per spec.

- [ ] **Step 3: Update OpenAPI spec**

In `docs/openapi.yaml`, add `dry_run` to the `IngestPathsRequest` schema properties (after the `collection` property, around line 315):

```yaml
        dry_run:
          type: boolean
          default: false
          description: >
            When true, validates paths, extracts text, and checks checksums
            without writing to Postgres or Qdrant. Chunks will always be 0
            in dry-run mode.
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace`
Expected: Success

- [ ] **Step 5: Commit**

```bash
git add crates/rag-server/src/routes/ingest.rs docs/openapi.yaml
git commit -m "feat(server): accept dry_run in /ingest request"
```

---

### Task 4: Client + CLI — wire `--dry-run` flag end to end

**Files:**
- Modify: `crates/rag-client/src/types.rs:11-16`
- Modify: `crates/rag-client/tests/client_tests.rs:131,192,339`
- Modify: `crates/rag-cli/src/cli.rs:44-50`
- Modify: `crates/rag-cli/src/commands/ingest.rs:9-36,79-135`
- Modify: `crates/rag-cli/src/main.rs:27-29`

- [ ] **Step 1: Write the failing CLI test — dry-run output prefix**

In `crates/rag-cli/src/commands/ingest.rs`, add a new test at the end of the `mod tests` block:

```rust
    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_dry_run_prefixes_output() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 5, chunks: 0, skipped: 2, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, true)
            .await
            .unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.starts_with("[dry-run]"),
            "dry-run output should be prefixed, got: {output}"
        );
        assert!(output.contains("5 documents"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-cli ingest_dry_run_prefixes_output -- --nocapture`
Expected: Compilation error — `run()` doesn't accept a 6th argument yet.

- [ ] **Step 3: Add `dry_run` to client `IngestRequest`**

In `crates/rag-client/src/types.rs`, update the struct at line 11:

```rust
#[derive(Debug, Serialize)]
pub struct IngestRequest {
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    pub dry_run: bool,
}
```

- [ ] **Step 4: Fix rag-client test construction sites**

In `crates/rag-client/tests/client_tests.rs`, add `dry_run: false` to all three `IngestRequest` literals:

Line 131 (`tenant_header_injected`):
```rust
            &IngestRequest {
                paths: vec!["/tmp/test.txt".into()],
                collection: None,
                dry_run: false,
            },
```

Line 192 (`ingest_request_shape`):
```rust
        .ingest(&IngestRequest {
            paths: vec!["/data/file.pdf".into()],
            collection: Some("docs".into()),
            dry_run: false,
        })
```

Also add an assertion for `dry_run` in the request body check at line ~205:
```rust
    assert_eq!(body["dry_run"], false);
```

Line 339 (`ingest_timeout_override`):
```rust
        .ingest(&IngestRequest {
            paths: vec!["/tmp/test.txt".into()],
            collection: None,
            dry_run: false,
        })
```

- [ ] **Step 5: Add `--dry-run` flag to CLI command**

In `crates/rag-cli/src/cli.rs`, update the `Ingest` variant at line 44:


```rust
    /// Ingest files and directories
    Ingest {
        /// Paths to ingest (files or directories, auto-detected)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        collection: Option<String>,
        /// Show what would be ingested without writing anything
        #[arg(long)]
        dry_run: bool,
    },
```

- [ ] **Step 6: Update CLI dispatch in `main.rs`**

In `crates/rag-cli/src/main.rs`, update the `Ingest` match arm at line 27:

```rust
        cli::Command::Ingest { paths, collection, dry_run } => {
            commands::ingest::run(&client, &mut stdout, cli.json, paths, collection, dry_run)
                .await
        }
```

- [ ] **Step 7: Update the `run()` function to accept `dry_run` and prefix output**

In `crates/rag-cli/src/commands/ingest.rs`, replace the entire `run()` function:

```rust
pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    paths: Vec<PathBuf>,
    collection: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let req = IngestRequest {
        paths: paths.iter().map(|p| p.display().to_string()).collect(),
        collection,
        dry_run,
    };

    let resp = client.ingest(&req).await?;

    let prefix = if dry_run { "[dry-run] " } else { "" };
    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(
            w,
            "{prefix}Ingested {} documents, {} chunks, {} skipped",
            resp.documents, resp.chunks, resp.skipped
        )?;
        for f in &resp.failures {
            writeln!(w, "WARN: {} — {}", f.path, f.error)?;
        }
        Ok(())
    })
}
```

- [ ] **Step 8: Fix existing CLI tests**

All existing tests call `run()` with 5 arguments. Add `false` as the 6th argument (`dry_run`) to each:

`ingest_human_output` (line ~86):
```rust
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, false)
            .await
            .unwrap();
```

`ingest_partial_failure` (line ~106):
```rust
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, false)
            .await
            .unwrap();
```

`ingest_json_output` (line ~118):
```rust
        run(&client, &mut buf, true, vec![PathBuf::from("/data/docs")], None, false)
            .await
            .unwrap();
```

`ingest_accepts_relative_paths` (line ~131):
```rust
        run(&client, &mut buf, false, vec![PathBuf::from("data/docs")], None, false)
            .await
            .unwrap();
```

- [ ] **Step 9: Add CLI parsing test for `--dry-run` flag**

In `crates/rag-cli/src/cli.rs`, add a test in the existing `mod tests` block:

```rust
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn ingest_accepts_dry_run_flag() {
        let cli = Cli::try_parse_from(["rag-cli", "ingest", "--dry-run", "/path"]);
        let cli = cli.expect("--dry-run should be accepted");
        match cli.command {
            Command::Ingest { dry_run, .. } => assert!(dry_run),
            _ => panic!("expected Ingest command"),
        }
    }
```

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test -p rag-cli -- --nocapture`
Expected: All ingest tests PASS (4 existing + 1 dry-run output test + 1 parsing test).

Run: `cargo test -p rag-client -- --nocapture`
Expected: All client tests PASS (construction sites updated).

- [ ] **Step 11: Verify full workspace compilation**

Run: `cargo check --workspace`
Expected: Success

- [ ] **Step 12: Commit**

```bash
git add crates/rag-client/src/types.rs crates/rag-client/tests/client_tests.rs crates/rag-cli/src/cli.rs crates/rag-cli/src/commands/ingest.rs crates/rag-cli/src/main.rs
git commit -m "feat(cli): add --dry-run flag to ingest command"
```
