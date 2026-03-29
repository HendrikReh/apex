use std::io::Write;
use std::path::PathBuf;

use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _paths: Vec<PathBuf>,
    _collection: Option<String>,
) -> anyhow::Result<()> {
    anyhow::bail!("ingest not yet implemented")
}
