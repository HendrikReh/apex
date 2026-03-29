# Apex

A Rust workspace for Retrieval-Augmented Generation (RAG). Ingests documents (PDF, Markdown, plain text), chunks with configurable strategies, generates embeddings (OpenAI or mock), and stores in Postgres (metadata) + Qdrant (vectors) for hybrid retrieval (dense + BM25 sparse).

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
rag-cli search "how does authentication work?" --collection my-docs

# Chat (requires OPENAI_API_KEY in .env for real LLM responses)
rag-cli chat "summarize the main concepts" --collection my-docs
```

### Using real embeddings

For production-quality retrieval, use OpenAI embeddings instead of mock:

```bash
# Add your API key to .env
echo 'OPENAI_API_KEY=sk-...' >> .env

# Run with OpenAI embeddings
just run-server
```

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
| `rag-cli` | Command-line interface (ingest, search, chat) |

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
| `.env` | Secrets | `OPENAI_API_KEY`, `DATABASE_URL` |
| Environment variables | Overrides | Take precedence over both files |

## Development

```bash
just test          # fmt + clippy + cargo test
just fmt           # Check formatting
just clippy        # Lint with strict settings
just up            # Start docker services
just down-v        # Stop services and wipe data
```

## License

[AGPL-3.0](LICENSE)
