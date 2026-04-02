#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_server::agents::AgentManager;
use rag_server::auth::oidc::JwksCache;
use rag_server::middleware::rate_limit::RateLimiterState;
use rag_server::router::build_router;
use rag_server::state::{AppState, AuthState};
use test_support::spawn_app;
use uuid::Uuid;

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn sample_fixture() -> PathBuf {
    workspace_root().join("crates/rag-server/tests/fixtures/sample.txt")
}

async fn full_app() -> axum::Router {
    // SAFETY: test-only env manipulation; this integration test runs in its
    // own process so there are no concurrent readers within this process.
    unsafe { std::env::set_var("RAG_EMBEDDER", "mock") };

    let template_path = workspace_root().join("config/prompts/chat_system.hbs");
    // SAFETY: same reasoning as above; this test process owns these env writes.
    unsafe { std::env::set_var("LLM_PROMPT_TEMPLATE_PATH", &template_path) };

    let config = AppConfig::from_env().expect("test config");
    let stores = Stores::new(&config).await.expect("test stores");
    let ingest = IngestService::new(stores.clone(), &config).expect("test ingest");
    let retrieval =
        Arc::new(RetrievalService::new(stores.clone(), &config).expect("test retrieval"));
    let chat = Arc::new(
        ChatService::with_mock_llm(stores.clone(), &config, "Mock LLM response.".into())
            .expect("test chat"),
    );
    let agents = Arc::new(
        AgentManager::load_default(
            &config.agent_specs_dir,
            stores.clone(),
            retrieval.clone(),
            chat.clone(),
        )
        .await
        .expect("agents"),
    );
    let tenant_header = config.tenant_header.parse().expect("tenant header");
    let auth =
        AuthState { jwks_cache: JwksCache::default(), rate_limiter: RateLimiterState::default() };
    let state =
        Arc::new(AppState { ingest, retrieval, chat, agents, stores, config, tenant_header, auth });

    build_router(state)
}

fn command_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

async fn run_cli(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_rag-cli")).args(&args).output().expect("run rag-cli")
    })
    .await
    .expect("join rag-cli task")
}

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_then_chat_via_cli_subprocess() {
    let server = spawn_app(full_app().await).await.expect("spawn");
    let fixture = sample_fixture();
    assert!(fixture.is_file(), "fixture missing: {}", fixture.display());

    let suffix = unique_suffix();
    let collection = format!("test-cli-e2e-coll-{suffix}");
    let tenant = format!("test-cli-e2e-{suffix}");
    let server_url = server.base_url();

    let ingest = run_cli(vec![
        "--server".into(),
        server_url.clone(),
        "--tenant".into(),
        tenant.clone(),
        "ingest".into(),
        fixture.display().to_string(),
        "--collection".into(),
        collection.clone(),
    ])
    .await;

    assert!(ingest.status.success(), "ingest failed\n{}", command_output(&ingest));
    let ingest_stdout = String::from_utf8(ingest.stdout).expect("ingest stdout utf-8");
    assert!(
        ingest_stdout.contains("Ingested 1 documents"),
        "unexpected ingest stdout:\n{ingest_stdout}"
    );

    let chat = run_cli(vec![
        "--server".into(),
        server_url,
        "--tenant".into(),
        tenant,
        "chat".into(),
        "--query".into(),
        "What is in the document?".into(),
        "--collection".into(),
        collection,
    ])
    .await;

    assert!(chat.status.success(), "chat failed\n{}", command_output(&chat));
    let chat_stdout = String::from_utf8(chat.stdout).expect("chat stdout utf-8");
    assert!(chat_stdout.contains("Mock LLM response."), "unexpected chat stdout:\n{chat_stdout}");
    assert!(chat_stdout.contains("[1]"), "expected at least one citation:\n{chat_stdout}");
    assert!(
        chat_stdout.contains("[conversation:"),
        "expected conversation metadata:\n{chat_stdout}"
    );
    assert!(chat_stdout.contains("[model: mock]"), "expected model metadata:\n{chat_stdout}");
}
