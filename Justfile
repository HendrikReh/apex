set dotenv-load := true

compose_file := "docker-compose.yml"

# Isolated Cargo target directories to avoid lock contention between
# rust-analyzer/background checks and terminal commands.
target_dir_check := "target/check"
target_dir_clippy := "target/clippy"
target_dir_test := "target/test"
target_dir_run := "target/run"

# Shared RUST_LOG presets.
rust_log_server := "info,rag_server=debug,rag_core=debug,tower_http=debug"

# ── Docker ────────────────────────────────────────────────────────────

# Start Postgres + Qdrant
up:
    docker compose -f {{compose_file}} up -d

# Stop services
down:
    docker compose -f {{compose_file}} down

# Stop services and remove volumes
down-v:
    docker compose -f {{compose_file}} down -v

# ── Build ─────────────────────────────────────────────────────────────

# Check formatting (no changes)
fmt:
    cargo fmt --all -- --check

# Clippy with strict settings
clippy:
    CARGO_TARGET_DIR={{target_dir_clippy}} \
        cargo clippy --workspace --all-targets -- -D warnings -D clippy::disallowed_methods

# ── Test ──────────────────────────────────────────────────────────────

# Full suite: fmt check -> clippy -> cargo test
test: fmt clippy
    CARGO_TARGET_DIR={{target_dir_test}} cargo test --workspace

# ── Server ────────────────────────────────────────────────────────────

# Run server with OpenAI embeddings (needs OPENAI_API_KEY)
run-server:
    CARGO_TARGET_DIR={{target_dir_run}} RUST_LOG={{rust_log_server}} \
        cargo run -p rag-server

# Run server with mock embeddings (no API key required)
run-server-mock:
    CARGO_TARGET_DIR={{target_dir_run}} RUST_LOG={{rust_log_server}} \
        RAG_EMBEDDER=mock cargo run -p rag-server
