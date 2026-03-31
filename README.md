# Apex

[![CI](https://github.com/HendrikReh/apex/actions/workflows/ci.yml/badge.svg)](https://github.com/HendrikReh/apex/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange)](https://www.rust-lang.org/)

Apex is a API-first, self-hostable Retrieval-Augmented Generation platform focused on grounded answers, clear system boundaries, and production-grade traceability. It ingests documents, builds hybrid retrieval indexes over Postgres and Qdrant, and exposes the workflow through an HTTP API plus a CLI.

Today, Apex already covers the core RAG path end to end:

- document ingest for PDF, Markdown, and plain text
- chunking with configurable strategies
- dense, sparse, and hybrid retrieval
- chat responses with citations
- multi-tenant HTTP and CLI workflows

## Note

> Apex is the open-source migration path for Apex Accelerator, my commercial offering. That migration is still in progress.
>
> The current open-source release is centered on Phase 1, the core RAG platform. Later phases bring over the observability, provenance, agent, and compliance layers that matter in enterprise deployments.

### Migration roadmap

1. **Phase 1: Open RAG foundation**
   Ingest, retrieval, chat, API, CLI, and the storage/runtime model needed for a solid self-hosted RAG stack.
2. **Phase 2: Observability and provenance**
   OpenTelemetry instrumentation, richer provenance capture, and better auditability for where answers came from and how they were produced.
3. **Phase 3: Agent workflows**
   Agent definitions, execution flows, tools, runs, and evidence-oriented orchestration on top of the RAG substrate.
4. **Phase 4: EU AI Act alignment**
   Compliance-facing controls, documentation, and governance features aimed at real-world high-assurance deployments.

Some commercial-only pieces remain out of scope for the open-source project or will land in different form, including the license server, binary obfuscation, and certain cloud-native infrastructure for authentication and persistence.

## Contents

- [Prerequisites](#prerequisites)
- [Quickstart](#quickstart)
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

# 4. Run the server with mock embeddings (no API key needed)
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

### Using a real LLM and embeddings

For grounded chat responses and production-quality retrieval, add your API key and run the default server profile:

```bash
# Add your API key to .env
echo 'OPENAI_API_KEY=sk-...' >> .env

# Run with OpenAI-compatible LLM + embeddings
just run-server

# Then ask a question through the CLI
rag-cli chat --query "Summarize the main concepts" --collection my-docs
```

## Why Apex

- **Rust end to end** for the server, client, and core retrieval pipeline.
- **Self-hostable architecture** with explicit storage boundaries: Postgres for metadata, Qdrant for vectors.
- **Grounded responses** with retrieval-backed citations instead of opaque chat completions.
- **Multi-tenant by design** through consistent tenant handling across API and CLI paths.
- **Built for migration to higher-assurance workflows** such as provenance, observability, agents, and compliance controls.

## Architecture

```
rag-cli  -->  rag-client  -->  HTTP  -->  rag-server  -->  rag-core  -->  rag-chunking
                                              |
                                         Postgres (metadata)
                                         Qdrant   (vectors)
```

| Crate | Role |
|-------|------|
| `rag-core` | Extraction, chunking, embedding, retrieval, context assembly, stores |
| `rag-chunking` | Token-aware text chunking strategies |
| `rag-server` | Axum HTTP API with multi-tenant middleware |
| `rag-client` | Typed HTTP client for the server API |
| `rag-cli` | Command-line interface for ingest, search, chat, and collection stats |

## API

The server exposes a JSON API on `http://localhost:8080` by default. See [`docs/openapi.yaml`](docs/openapi.yaml) for the full specification.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe |
| `/readiness` | GET | Readiness probe (checks Postgres + Qdrant) |
| `/ingest` | POST | Ingest documents from filesystem paths |
| `/ingest/upload` | POST | Upload and ingest a single file (multipart) |
| `/search/dense` | POST | Dense vector search |
| `/search/sparse` | POST | BM25 sparse search |
| `/search/hybrid` | POST | Hybrid search with RRF fusion |
| `/chat` | POST | RAG chat with citations |
| `/collections/:name/stats` | GET | Collection statistics |

Multi-tenancy is built in. Pass `x-tenant` header to isolate data per tenant (defaults to `"default"`).

## Configuration

| Source | Purpose | Example |
|--------|---------|---------|
| `config/app.toml` | Non-secret settings | Chunking params, endpoints, model names |
| `.env` | Secrets and local overrides | `OPENAI_API_KEY`, `DATABASE_URL` |
| Environment variables | Overrides | Take precedence over both files |

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
