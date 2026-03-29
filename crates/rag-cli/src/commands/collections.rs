use std::io::Write;

use rag_client::ApiClient;

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    collection: String,
) -> anyhow::Result<()> {
    let resp = client.collection_stats(&collection).await?;

    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(w, "Collection: {}", resp.collection)?;
        writeln!(w, "Tenant:     {}", resp.tenant)?;
        writeln!(w, "Documents:  {}", resp.total_docs)?;
        writeln!(w, "Tokens:     {}", resp.total_tokens)?;
        writeln!(w, "Avg Doc Length: {:.2}", resp.avgdl)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;

    struct FakeCollectionsClient;

    impl rag_client::ApiClient for FakeCollectionsClient {
        async fn health(&self) -> Result<(), ClientError> {
            Ok(())
        }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> {
            unimplemented!()
        }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> {
            unimplemented!()
        }
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            unimplemented!()
        }
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            unimplemented!()
        }
        async fn search_hybrid(
            &self,
            _: &HybridSearchRequest,
        ) -> Result<HybridSearchResponse, ClientError> {
            unimplemented!()
        }
        async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ClientError> {
            unimplemented!()
        }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> {
            Ok(CollectionStatsResponse {
                collection: "my-collection".into(),
                tenant: "default".into(),
                total_docs: 42,
                total_tokens: 12345,
                avgdl: 293.93,
            })
        }
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn collection_stats_human_output() {
        let client = FakeCollectionsClient;
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "my-collection".into()).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Collection: my-collection"));
        assert!(output.contains("Documents:  42"));
        assert!(output.contains("Tokens:     12345"));
        assert!(output.contains("Avg Doc Length: 293.93"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn collection_stats_json_output() {
        let client = FakeCollectionsClient;
        let mut buf = Vec::new();
        run(&client, &mut buf, true, "my-collection".into()).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["total_docs"], 42);
        assert_eq!(parsed["avgdl"], 293.93);
    }
}
