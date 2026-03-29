---
name: add-crate
description: Scaffold a new crate in the workspace with correct Cargo.toml, lib.rs conventions, and workspace registration. Use when the user wants to create a new crate, add a new module as a separate crate, or split functionality into its own crate.
---

# Add Workspace Crate

Scaffold a new crate in the apex workspace, following all project conventions.

## Information Needed

Ask the user for:
- **Crate name** (e.g., `rag-auth`, `agent-memory`)
- **Crate type**: library or binary
- **Brief description** of what the crate does
- **Dependencies** (from workspace or external)

## Step 1: Create Crate Structure

```bash
mkdir -p crates/<crate-name>/src
```

### Cargo.toml (library crate)
```toml
[package]
name = "<crate-name>"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
# Workspace deps — use { workspace = true } for all shared dependencies
anyhow = { workspace = true }
serde = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

### Cargo.toml (binary crate)
```toml
[package]
name = "<crate-name>"
version.workspace = true
edition.workspace = true
publish = false

[[bin]]
name = "<crate-name>"
path = "src/main.rs"

[dependencies]
anyhow = { workspace = true }
```

### Adding Dependencies

**Workspace dependency** (already in root Cargo.toml `[workspace.dependencies]`):
```toml
serde = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread"] }
```

**New external dependency** — add to root Cargo.toml first, then reference:
```toml
# In root Cargo.toml [workspace.dependencies]:
new-crate = "1.2"

# In your crate's Cargo.toml:
new-crate = { workspace = true }
```

**Internal workspace dependency**:
```toml
rag-core = { path = "../rag-core" }
```

## Step 2: Create Source Files

### lib.rs (library crates)
```rust
//! Brief description of the crate.
#![deny(unsafe_code)]
```

If tests will use `.unwrap()` (most do):
```rust
//! Brief description of the crate.
#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods))]
```

### main.rs (binary crates)
```rust
use anyhow::Result;

fn main() -> Result<()> {
    // ...
    Ok(())
}
```

## Step 3: Register in Workspace

Add the crate path to root `Cargo.toml` under `[workspace] members`, maintaining alphabetical order within the existing sections.

## Step 4: Verify

```bash
cargo check -p <crate-name>
```

## Crate Boundary Rules

**Library crates** (`crates/` directory):
- Must NOT use `println!`, `eprintln!`, `print!`, `eprint!`
- Must NOT call `std::process::exit`
- All errors as `Result` types — propagate with `?` + `anyhow::Context`
- No `.unwrap()` or `.expect()` in production code (ADR-001)

**Binary crates** (`crates/` with `[[bin]]`):
- May write to console
- Still use `Result` for error propagation where possible

## Dependency Flow (do not violate)

```
rag-cli -> rag-client -> reqwest (HTTP only)
rag-server -> rag-core -> rag-chunking
rag-server -> agent-core
rag-server -> rag-evidence, rag-notifications
rag-cli -> rag-client, rag-evidence
obfuscate-macros: proc-macro, no workspace deps
test-support: test utilities only, no workspace deps
```

New crates should fit into this hierarchy or extend it at leaf positions. Do NOT create circular dependencies.

## Checklist

- [ ] Directory created at `crates/<crate-name>/src/`
- [ ] `Cargo.toml` with `{ workspace = true }` for version, edition, and shared deps
- [ ] `src/lib.rs` (or `src/main.rs`) with `#![deny(unsafe_code)]` for libraries
- [ ] Registered in root `Cargo.toml` `[workspace] members` (alphabetical)
- [ ] If new external dep: added to root `[workspace.dependencies]`
- [ ] `cargo check -p <crate-name>` passes
- [ ] Dependency direction verified (libraries must not depend on binaries)

## Cross-references

- `/add-endpoint` — if the new crate is a server extension
