use std::io::Write;

use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _collection: String,
) -> anyhow::Result<()> {
    anyhow::bail!("collection-stats not yet implemented")
}
