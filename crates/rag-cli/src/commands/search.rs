use std::io::Write;

use crate::cli::SearchMode;
use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _query: String,
    _collection: String,
    _mode: SearchMode,
    _top_k: Option<u64>,
) -> anyhow::Result<()> {
    anyhow::bail!("search not yet implemented")
}
