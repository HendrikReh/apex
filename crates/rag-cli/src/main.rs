mod cli;
mod commands;
mod output;

use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();
    if let Err(e) = cli::validate(&cli) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    if let Err(e) = run(cli).await {
        eprintln!("{}", output::render_error(&e));
        std::process::exit(1);
    }
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    let client = rag_client::TenantApiClient::new(&cli.server, &cli.tenant)?;
    let mut stdout = std::io::stdout().lock();

    match cli.command {
        cli::Command::Ingest { paths, collection } => {
            commands::ingest::run(&client, &mut stdout, cli.json, paths, collection).await
        }
        cli::Command::Search {
            query,
            collection,
            mode,
            top_k,
        } => {
            commands::search::run(&client, &mut stdout, cli.json, query, collection, mode, top_k)
                .await
        }
        cli::Command::Chat {
            query,
            collection,
            interactive,
            conversation_id,
        } => {
            commands::chat::run(
                &client,
                &mut stdout,
                cli.json,
                query,
                collection,
                interactive,
                conversation_id,
            )
            .await
        }
        cli::Command::CollectionStats { collection } => {
            commands::collections::run(&client, &mut stdout, cli.json, collection).await
        }
    }
}
