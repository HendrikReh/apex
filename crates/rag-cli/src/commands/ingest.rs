use std::io::Write;
use std::path::PathBuf;

use rag_client::ApiClient;
use rag_client::types::IngestRequest;

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    paths: Vec<PathBuf>,
    collection: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let req = IngestRequest {
        paths: paths.iter().map(|p| p.display().to_string()).collect(),
        collection,
        dry_run,
    };

    let resp = client.ingest(&req).await?;

    let prefix = if dry_run { "[dry-run] " } else { "" };
    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(
            w,
            "{prefix}Ingested {} documents, {} chunks, {} skipped",
            resp.documents, resp.chunks, resp.skipped
        )?;
        for f in &resp.failures {
            writeln!(w, "WARN: {} — {}", f.path, f.error)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::path::PathBuf;

    struct FakeIngestClient {
        response: IngestResponse,
    }

    impl rag_client::ApiClient for FakeIngestClient {
        async fn health(&self) -> Result<(), ClientError> {
            Ok(())
        }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> {
            unimplemented!()
        }
        async fn ingest(&self, _req: &IngestRequest) -> Result<IngestResponse, ClientError> {
            Ok(self.response.clone())
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
            unimplemented!()
        }
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_human_output() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 3, chunks: 10, skipped: 1, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, false).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Ingested 3 documents, 10 chunks, 1 skipped"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_partial_failure() {
        let client = FakeIngestClient {
            response: IngestResponse {
                documents: 2,
                chunks: 5,
                skipped: 0,
                failures: vec![IngestFailure {
                    path: "/tmp/bad.txt".into(),
                    error: "parse error".into(),
                }],
            },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, false).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("WARN: /tmp/bad.txt — parse error"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_json_output() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 1, chunks: 2, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, true, vec![PathBuf::from("/data/docs")], None, false).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["documents"], 1);
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_accepts_relative_paths() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 1, chunks: 2, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        // Relative paths should be passed through to the server as-is
        run(&client, &mut buf, false, vec![PathBuf::from("data/docs")], None, false).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Ingested 1 documents"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_dry_run_prefixes_output() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 5, chunks: 0, skipped: 2, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![PathBuf::from("/data/docs")], None, true)
            .await
            .unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.starts_with("[dry-run]"),
            "dry-run output should be prefixed, got: {output}"
        );
        assert!(output.contains("5 documents"));
    }
}
