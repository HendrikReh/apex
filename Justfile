set dotenv-load := true

compose_file := "docker-compose.yml"

# Isolated Cargo target directories to avoid lock contention between
# rust-analyzer/background checks and terminal commands.
target_dir_check := "target/check"
target_dir_clippy := "target/clippy"
target_dir_test := "target/test"
target_dir_itest := "target/itest"
target_dir_smoke := "target/smoke"
target_dir_run := "target/run"

# Shared RUST_LOG presets.
rust_log_server := "info,rag_server=debug,rag_core=debug,tower_http=debug"

# ── Docker ────────────────────────────────────────────────────────────

# Start Postgres + Qdrant
up:
    docker compose -p ${APEX_COMPOSE_PROJECT_NAME:-apex} -f {{compose_file}} up -d

# Stop services
down:
    docker compose -p ${APEX_COMPOSE_PROJECT_NAME:-apex} -f {{compose_file}} down

# Stop services and remove volumes
down-v:
    docker compose -p ${APEX_COMPOSE_PROJECT_NAME:-apex} -f {{compose_file}} down -v

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

# Fast crate-local unit tests (`src/*`, plus testable binary crates)
unit-test:
    CARGO_TARGET_DIR={{target_dir_test}} cargo test --workspace --lib --bins

# Integration tests and API smoke against local Postgres + Qdrant.
# Starts Docker infra first and then runs the repo's non-provider integration suites.
integration-test: up
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-client --test client_tests -- --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-cli --test e2e -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test health -- --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test health -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test health_degraded -- --ignored --nocapture --test-threads=1
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test ingest -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test search -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test e2e -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-core --test integration_conversations -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-core --test integration_ingest -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-core --test integration_lifecycle -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-core --test integration_retrieval -- --ignored --nocapture

# Offline benchmark for routed agentic search.
agentic-eval: up
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test agentic_eval -- --ignored --nocapture --test-threads=1

# Optional provider/native smoke tests.
# These require extra local setup such as PDFium, Tesseract, or live LLM credentials.
smoke-test: up
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_smoke}} cargo test -p rag-core --test integration_pdf -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_smoke}} cargo test -p rag-core --test integration_chat -- --ignored --nocapture
    APEX_COMPOSE_PROJECT_NAME=${APEX_COMPOSE_PROJECT_NAME:-apex} CARGO_TARGET_DIR={{target_dir_smoke}} cargo test -p rag-core --test smoke_llm -- --ignored --nocapture

# ── Server ────────────────────────────────────────────────────────────

# Run server with OpenAI embeddings (needs OPENAI_API_KEY)
run-server:
    CARGO_TARGET_DIR={{target_dir_run}} RUST_LOG={{rust_log_server}} \
        cargo run -p rag-server

# Run server with mock embeddings (no API key required)
run-server-mock:
    CARGO_TARGET_DIR={{target_dir_run}} RUST_LOG={{rust_log_server}} \
        RAG_EMBEDDER=mock cargo run -p rag-server
