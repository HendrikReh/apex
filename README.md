# Apex

[![CI](https://github.com/HendrikReh/apex/actions/workflows/ci.yml/badge.svg)](https://github.com/HendrikReh/apex/actions/workflows/ci.yml)
[![Version](https://img.shields.io/badge/version-0.10.1-2ea44f?logo=github)](https://github.com/HendrikReh/apex)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](https://www.rust-lang.org/)
[![Roadmap](https://img.shields.io/badge/Roadmap-Phase%202%20--%20Agent%20Workflows-0a7ea4)](#migration-roadmap)

Apex is an API-first, self-hostable Retrieval-Augmented Generation platform focused on grounded answers, clear system boundaries, and production-grade traceability. It ingests documents, builds hybrid retrieval indexes over Postgres and Qdrant, and exposes the workflow through an HTTP API plus a CLI.

Today, the open-source project covers the core RAG path end to end:

- document ingest for PDF, Markdown, and plain text
- chunking with configurable strategies
- dense, sparse, and hybrid retrieval
- chat responses with citations
- authentication (API key + OIDC), role-based access control, and rate limiting
- multi-tenant HTTP and CLI workflows

The current Phase 1 platform is also hardened in the areas that matter operationally:

- protected routes now fail closed by default
- `/ingest` rejects invalid or out-of-bounds filesystem paths as `400` requests
- search keeps client-actionable collection errors visible while sanitizing real backend faults
- secrets in `rag-core` config are redacted from debug output
- `agent-core` validates tool references and rejects ambiguous checkpoint configs at load time
- the workspace is aligned on Rust 2024 formatting and denies `unsafe` in the core library crates

## Project Status

> Apex is the open-source migration path for Apex Accelerator, my commercial offering. That migration is still in progress.
>
> The current open-source release is centered on Phase 1, the core RAG platform. Later phases bring over the observability, provenance, agent, and compliance layers that matter in enterprise deployments.

### Migration roadmap

1. **Phase 1: Open RAG foundation**
   Ingest, retrieval, chat, API, CLI, and the storage/runtime model needed for a solid self-hosted RAG stack.
2. **Phase 2: Agent workflows**
   Agent definitions, execution flows, tools, runs, and evidence-oriented orchestration on top of the RAG substrate.
3. **Phase 3: Observability and provenance**
   OpenTelemetry instrumentation, richer provenance capture, and better auditability for where answers came from and how they were produced.
4. **Phase 4: EU AI Act alignment**
   Compliance-facing controls, documentation, and governance features aimed at real-world high-assurance deployments.

Some commercial-only pieces remain out of scope for the open-source project or will land in different form, including the license server, binary obfuscation, and certain cloud-native infrastructure for authentication and persistence.

## Contents

- [Prerequisites](#prerequisites)
- [Quickstart](#quickstart)
- [Running Modes](#running-modes)
- [Why Apex](#why-apex)
- [Architecture](#architecture)
- [API](#api)
- [Configuration](#configuration)
- [Development](#development)
- [License](#license)

## Prerequisites

- **Rust** 1.94+ (edition 2024)
- **Docker** (for Postgres + Qdrant)
- **[just](https://github.com/casey/just)** task runner

## Quickstart

```bash
# 1. Clone and enter the repo
git clone https://github.com/HendrikReh/apex.git
cd apex

# 2. Start Postgres + Qdrant
just up

# 3. Create a .env with your database URL (matches docker-compose defaults)
echo 'DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres' > .env

# 4. Run the server with mock embeddings for local development
just run-server-mock

# 5. In another terminal, install the CLI and try it out
cargo install --path crates/rag-cli

# Ingest some files
rag-cli ingest ./path/to/docs --collection my-docs

# Search
rag-cli search --query "how does authentication work?" --collection my-docs

# Collection statistics
rag-cli collection-stats --collection my-docs
```

`just run-server-mock` is useful for local ingestion and retrieval development because it removes the embedding dependency. Chat still needs a configured LLM provider.

## Running Modes

### Local development mode

Use `just run-server-mock` when you want a fast local loop for ingest and retrieval work without configuring embeddings. This is the easiest way to validate API, chunking, ingest, and search behavior on a fresh checkout.

### Real provider mode

For grounded chat responses and production-quality retrieval, add your API key and run the default server profile:

```bash
# Add your API key to .env
echo 'OPENAI_API_KEY=sk-...' >> .env

# Run with OpenAI-compatible LLM + embeddings
just run-server

# Then ask a question through the CLI
rag-cli chat --query "Summarize the main concepts" --collection my-docs
```

In real deployments, prefer provider-backed embeddings plus explicit filesystem boundaries for server-side ingestion.

## Why Apex

- **Rust end to end** for the server, client, and core retrieval pipeline.
- **Self-hostable architecture** with explicit storage boundaries: Postgres for metadata, Qdrant for vectors.
- **Grounded responses** with retrieval-backed citations instead of opaque chat completions.
- **Multi-tenant by design** through consistent tenant handling across API and CLI paths.
- **Built for migration to higher-assurance workflows** such as provenance, observability, agents, and compliance controls.

## Architecture

```
rag-cli ──→ rag-client ──→ HTTP ──→ rag-server ──→ rag-core ──→ rag-chunking
                                         │
                                         ├──→ agent-core (graph-flow DAG runner)
                                         │
                                    ┌────┴──────┐
                                    │ Middleware│
                                    │ stack:    │
                                    │ req-id    │
                                    │ tenant    │
                                    │ auth      │
                                    │ rate-limit│
                                    │ authz     │
                                    └─────┬─────┘
                                          │
                                   ┌──────┴──────┐
                                   │             │
                               Postgres       Qdrant
                              (metadata)     (vectors)
```

| Crate | Role |
|-------|------|
| `rag-core` | Extraction, embedding, retrieval, context assembly, stores, config |
| `rag-chunking` | 9 token-aware text chunking strategies (pure, no IO) |
| `agent-core` | Graph-flow DAG runner for agent orchestration (YAML-driven specs, checkpoints) |
| `rag-server` | Axum HTTP API — auth, RBAC, rate limiting, multi-tenant middleware |
| `rag-client` | Typed HTTP client for the server API |
| `rag-cli` | CLI for ingest, search, chat, stats, and API key generation |
| `test-support` | Ephemeral Axum test servers for integration tests |

## API

The server exposes a JSON API on `http://localhost:8080` by default. Interactive documentation is served at `/swagger-ui/` when the server is running.

| Endpoint | Method | Auth | Description |
|----------|--------|------|-------------|
| `/health` | GET | No | Liveness probe |
| `/readiness` | GET | No | Readiness probe (checks Postgres + Qdrant) |
| `/openapi.json` | GET | No | OpenAPI 3.1 specification |
| `/swagger-ui/` | GET | No | Interactive API documentation |
| `/ingest` | POST | Yes | Ingest documents from filesystem paths |
| `/ingest/upload` | POST | Yes | Upload and ingest a file (multipart, 50 MB) |
| `/search/dense` | POST | Yes | Dense vector search |
| `/search/sparse` | POST | Yes | BM25 sparse search |
| `/search/hybrid` | POST | Yes | Hybrid search with RRF fusion |
| `/chat` | POST | Yes | RAG chat with citations |
| `/collections/:name/stats` | GET | Yes | Collection statistics |
| `/auth/service-accounts` | POST | Yes | Create service account |
| `/auth/api-keys` | POST | Yes | Generate API key |
| `/auth/api-keys` | GET | Yes | List API keys |
| `/auth/api-keys/:id` | DELETE | Yes | Revoke API key |

Multi-tenancy is built in. Pass `x-tenant` header to isolate data per tenant (defaults to `"default"`).

## Configuration

| Source | Purpose | Example |
|--------|---------|---------|
| `config/app.toml` | Non-secret settings | Chunking params, endpoints, model names |
| `.env` | Secrets and local overrides | `OPENAI_API_KEY`, `DATABASE_URL` |
| Environment variables | Overrides | Take precedence over both files |

Two configuration boundaries are worth calling out explicitly:

- `ingest_allowed_roots` in `config/app.toml` controls which server-visible filesystem paths `/ingest` may access. Requests outside those roots are rejected with `400`.
- `/ingest/upload` is the safer choice when the client should send file content directly instead of referencing a path on the server host.

In practice:

```toml
# config/app.toml
ingest_allowed_roots = [
  "/srv/apex/import",
  "/var/lib/apex/dropbox",
]
```

Keep secrets such as `OPENAI_API_KEY` and `DATABASE_URL` in `.env` or environment variables rather than in `config/app.toml`.

## Development

```bash
just test          # fmt + clippy + cargo test
just unit-test     # fast unit-only loop
just integration-test  # Docker-backed integration suites
just smoke-test    # optional provider/native smoke suites
just fmt           # Check formatting
just clippy        # Lint with strict settings
just up            # Start docker services
just down-v        # Stop services and wipe data
```

See [docs/howto/testing.md](docs/howto/testing.md) for the full testing workflow, where to place tests, prerequisites, and concrete examples.

## License

[AGPL-3.0](LICENSE)
