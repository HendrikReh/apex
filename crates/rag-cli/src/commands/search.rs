use std::io::Write;

use rag_client::ApiClient;
use rag_client::types::{HybridSearchRequest, HybridSearchResponse, SearchRequest, SearchResponse};

use crate::cli::SearchMode;
use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    query: String,
    collection: String,
    mode: SearchMode,
    top_k: Option<u64>,
) -> anyhow::Result<()> {
    match mode {
        SearchMode::Dense => {
            let req = SearchRequest { query, collection, top_k };
            let resp = client.search_dense(&req).await?;
            print_or_json(writer, json, &resp, format_search)
        }
        SearchMode::Sparse => {
            let req = SearchRequest { query, collection, top_k };
            let resp = client.search_sparse(&req).await?;
            print_or_json(writer, json, &resp, format_search)
        }
        SearchMode::Hybrid => {
            let req = HybridSearchRequest {
                query,
                collection,
                dense_top_k: top_k,
                sparse_top_k: top_k,
                rrf_k: None,
            };
            let resp = client.search_hybrid(&req).await?;
            print_or_json(writer, json, &resp, format_hybrid)
        }
    }
}

fn format_search<W: Write>(resp: &SearchResponse, w: &mut W) -> anyhow::Result<()> {
    if resp.results.is_empty() {
        writeln!(w, "No results found.")?;
        return Ok(());
    }
    for (i, r) in resp.results.iter().enumerate() {
        let preview: String = r.text.chars().take(80).collect();
        writeln!(
            w,
            "[{}] (score: {:.2}) {} — {}, chunk {}",
            i + 1,
            r.score,
            r.chunk_id,
            r.document_id,
            r.chunk_index
        )?;
        writeln!(w, "    {preview}")?;
    }
    Ok(())
}

fn format_hybrid<W: Write>(resp: &HybridSearchResponse, w: &mut W) -> anyhow::Result<()> {
    if resp.results.is_empty() {
        writeln!(w, "No results found.")?;
        return Ok(());
    }
    for (i, r) in resp.results.iter().enumerate() {
        let preview: String = r.text.chars().take(80).collect();
        writeln!(
            w,
            "[{}] (score: {:.2}) {} — {}, chunk {}",
            i + 1,
            r.fused_score,
            r.chunk_id,
            r.document_id,
            r.chunk_index
        )?;
        writeln!(w, "    {preview}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::sync::Mutex;

    struct FakeSearchClient {
        called: Mutex<Vec<String>>,
    }

    impl FakeSearchClient {
        fn new() -> Self {
            Self { called: Mutex::new(Vec::new()) }
        }
    }

    impl rag_client::ApiClient for FakeSearchClient {
        async fn health(&self) -> Result<(), ClientError> {
            Ok(())
        }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> {
            unimplemented!()
        }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> {
            unimplemented!()
        }
        #[allow(clippy::disallowed_methods)]
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            self.called.lock().unwrap().push("dense".into());
            Ok(SearchResponse {
                results: vec![SearchResult {
                    chunk_id: "c1".into(),
                    document_id: "d1".into(),
                    chunk_index: 0,
                    text: "hello world".into(),
                    score: 0.8,
                }],
            })
        }
        #[allow(clippy::disallowed_methods)]
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            self.called.lock().unwrap().push("sparse".into());
            Ok(SearchResponse { results: vec![] })
        }
        #[allow(clippy::disallowed_methods)]
        async fn search_hybrid(
            &self,
            _: &HybridSearchRequest,
        ) -> Result<HybridSearchResponse, ClientError> {
            self.called.lock().unwrap().push("hybrid".into());
            Ok(HybridSearchResponse {
                results: vec![HybridSearchResult {
                    chunk_id: "c1".into(),
                    document_id: "d1".into(),
                    chunk_index: 0,
                    text: "hello world".into(),
                    fused_score: 0.9,
                }],
            })
        }
        async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ClientError> {
            unimplemented!()
        }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> {
            unimplemented!()
        }
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn search_hybrid_human_output() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "test".into(), "coll".into(), SearchMode::Hybrid, None)
            .await
            .unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[1] (score: 0.90)"));
        assert!(output.contains("hello world"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn search_dense_dispatches_correctly() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "test".into(), "coll".into(), SearchMode::Dense, None)
            .await
            .unwrap();
        let called = client.called.lock().unwrap();
        assert_eq!(called.as_slice(), &["dense"]);
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn search_json_output() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, true, "test".into(), "coll".into(), SearchMode::Hybrid, None)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(parsed["results"].is_array());
    }
}
