# Architecture Boundary Checker

Verify crate dependency rules and module boundaries for changed code.

## Rules

### Dependency Flow (strict)
- rag-cli -> rag-client -> reqwest (ONLY)
- rag-server -> rag-core -> rag-chunking
- rag-server -> agent-core (post-MVP)
- rag-server -> rag-evidence (post-MVP)
- rag-server -> rag-notifications (post-MVP)
- agent-core -> rag-core (post-MVP)
- rag-cli -> rag-client, rag-evidence
- obfuscate-macros: proc-macro only — no workspace deps
- test-support: test utilities only — no production logic, no workspace deps

### Forbidden Patterns in Library Crates
Library crates (rag-core, rag-chunking, agent-core, rag-client, rag-evidence, rag-notifications, rag-sbom):
- No `println!`, `eprintln!`, `print!`, `eprint!`
- No `std::process::exit`
- No direct `std::io::stdout()` or `std::io::stderr()` writes
- All errors as `Result` types (never panic in production paths)

### Cross-Crate Imports
- No circular dependencies
- rag-core must NOT import from rag-server
- rag-client must NOT import from rag-core (it's HTTP-only)
- rag-chunking must NOT import from any workspace crate (pure logic + tiktoken-rs)
- obfuscate-macros must NOT import from any workspace crate
- test-support must NOT import from any workspace crate

## Process
1. Identify changed crates from the diff
2. Check each changed file's imports against the dependency flow
3. Scan for forbidden patterns in library crates
4. Verify no new cross-crate dependency violations
5. Report: file, violation, rule, fix
