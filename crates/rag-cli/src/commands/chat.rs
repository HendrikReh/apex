use std::io::Write;

use rag_client::ApiClient;
use uuid::Uuid;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _query: String,
    _collection: Option<String>,
    _interactive: bool,
    _conversation_id: Option<Uuid>,
) -> anyhow::Result<()> {
    anyhow::bail!("chat not yet implemented")
}
