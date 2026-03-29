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
    // Canonicalize paths (resolves relative paths and symlinks, also verifies existence)
    let mut resolved = Vec::with_capacity(paths.len());
    for p in &paths {
        let canonical = p
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("path does not exist or is inaccessible: {} — {e}", p.display()))?;
        resolved.push(canonical);
    }

    let req = IngestRequest {
        paths: resolved.iter().map(|p| p.display().to_string()).collect(),
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
    async fn ingest_rejects_nonexistent_relative_path() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 0, chunks: 0, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        let result =
            run(&client, &mut buf, false, vec![PathBuf::from("relative/nonexistent")], None).await;
        assert!(result.is_err());
        assert!(result.expect_err("should fail").to_string().contains("does not exist"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn ingest_canonicalizes_relative_path() {
        let tmp = temp_file();
        // Get the directory containing the temp file and construct a relative-looking absolute path
        let abs_path = tmp.path().to_path_buf();
        let client = FakeIngestClient {
            response: IngestResponse { documents: 1, chunks: 2, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        // This should succeed — canonicalize resolves the path
        run(&client, &mut buf, false, vec![abs_path], None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Ingested 1 documents"));
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
