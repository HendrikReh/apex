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
) -> anyhow::Result<()> {
    // Validate paths before sending
    for p in &paths {
        if !p.is_absolute() {
            anyhow::bail!("all paths must be absolute, got: {}", p.display());
        }
        if !p.exists() {
            anyhow::bail!("path does not exist: {}", p.display());
        }
    }

    let req = IngestRequest {
        paths: paths.iter().map(|p| p.display().to_string()).collect(),
        collection,
    };

    let resp = client.ingest(&req).await?;

    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(
            w,
            "Ingested {} documents, {} chunks, {} skipped",
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

    /// Create a temporary file with an absolute path for testing.
    #[allow(clippy::disallowed_methods)]
    fn temp_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().unwrap()
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_human_output() {
        let tmp = temp_file();
        let client = FakeIngestClient {
            response: IngestResponse { documents: 3, chunks: 10, skipped: 1, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Ingested 3 documents, 10 chunks, 1 skipped"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_partial_failure() {
        let tmp = temp_file();
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
        // run() should succeed (partial failure is not a command error)
        run(&client, &mut buf, false, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("WARN: /tmp/bad.txt — parse error"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_json_output() {
        let tmp = temp_file();
        let client = FakeIngestClient {
            response: IngestResponse { documents: 1, chunks: 2, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, true, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["documents"], 1);
    }

    #[tokio::test]
    async fn ingest_rejects_relative_path() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 0, chunks: 0, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        let result =
            run(&client, &mut buf, false, vec![PathBuf::from("relative/path")], None).await;
        assert!(result.is_err());
        assert!(result.expect_err("should fail").to_string().contains("absolute"));
    }

    #[tokio::test]
    async fn ingest_rejects_nonexistent_path() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 0, chunks: 0, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        let result =
            run(&client, &mut buf, false, vec![PathBuf::from("/nonexistent/path/xyz")], None).await;
        assert!(result.is_err());
        assert!(result.expect_err("should fail").to_string().contains("does not exist"));
    }
}
