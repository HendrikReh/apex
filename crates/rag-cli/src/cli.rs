//! CLI argument parsing.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use uuid::Uuid;

#[derive(Parser)]
#[command(name = "rag-cli", about = "CLI for the Apex RAG server")]
pub struct Cli {
    /// Server URL
    #[arg(long, env = "RAG_SERVER_URL", default_value = "http://127.0.0.1:8080")]
    pub server: String,

    /// Tenant identifier
    #[arg(long, env = "RAG_TENANT", default_value = "default")]
    pub tenant: String,

    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// RAG chat (single-shot or interactive)
    Chat {
        #[arg(long)]
        query: String,
        /// Required for first message; omit when resuming via --conversation-id
        #[arg(long)]
        collection: Option<String>,
        /// Enter interactive multi-turn mode
        #[arg(long)]
        interactive: bool,
        /// Resume an existing conversation
        #[arg(long)]
        conversation_id: Option<Uuid>,
    },
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
    /// Search for chunks (dense, sparse, or hybrid)
    Search {
        #[arg(long)]
        query: String,
        #[arg(long)]
        collection: String,
        /// Search mode
        #[arg(long, value_enum, default_value_t = SearchMode::Hybrid)]
        mode: SearchMode,
        #[arg(long)]
        top_k: Option<u64>,
    },
    /// Show collection statistics
    #[command(name = "collection-stats")]
    CollectionStats {
        #[arg(long)]
        collection: String,
    },
    /// API key management
    #[command(name = "api-key")]
    ApiKey {
        #[command(subcommand)]
        action: ApiKeyAction,
    },
}

#[derive(Subcommand)]
pub enum ApiKeyAction {
    /// Generate a new API key locally (does not contact the server)
    Generate,
}

#[derive(Clone, ValueEnum)]
pub enum SearchMode {
    Dense,
    Sparse,
    Hybrid,
}

/// Validate CLI constraints that Clap cannot express declaratively.
pub fn validate(cli: &Cli) -> anyhow::Result<()> {
    if let Command::Chat { interactive, .. } = &cli.command
        && *interactive
        && cli.json
    {
        anyhow::bail!("--interactive and --json cannot be used together");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn ingest_rejects_no_paths() {
        let result = Cli::try_parse_from(["rag-cli", "ingest"]);
        assert!(result.is_err());
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn interactive_json_conflict() {
        let cli = Cli::try_parse_from([
            "rag-cli",
            "--json",
            "chat",
            "--query",
            "hi",
            "--collection",
            "c",
            "--interactive",
        ]);
        // Parsing succeeds, but validation fails
        let cli = cli.expect("should parse");
        assert!(validate(&cli).is_err());
    }

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

    #[allow(clippy::disallowed_methods)]
    #[serial_test::serial]
    #[test]
    fn server_env_fallback() {
        // Set env var before parsing — serial to avoid racing with other tests
        unsafe { std::env::set_var("RAG_SERVER_URL", "http://my-server:9090") };
        let cli = Cli::try_parse_from(["rag-cli", "collection-stats", "--collection", "test"])
            .expect("should parse");
        assert_eq!(cli.server, "http://my-server:9090");
        unsafe { std::env::remove_var("RAG_SERVER_URL") };
    }
}
