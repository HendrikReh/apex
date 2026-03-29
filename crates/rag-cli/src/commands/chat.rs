use std::io::{BufRead, Write};

use uuid::Uuid;

use rag_client::ApiClient;
use rag_client::types::{ChatRequest, ChatResponse};

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    query: String,
    collection: Option<String>,
    interactive: bool,
    conversation_id: Option<Uuid>,
) -> anyhow::Result<()> {
    // Validate: collection required for new conversations
    if collection.is_none() && conversation_id.is_none() {
        anyhow::bail!("--collection is required for the first message in a conversation");
    }

    let req = ChatRequest {
        query,
        collection: collection.clone(),
        conversation_id,
        language: None,
        history_limit: None,
    };

    let resp = client.chat(&req).await?;

    if !interactive {
        return print_or_json(writer, json, &resp, format_chat);
    }

    // Interactive mode: print first response, then loop
    format_chat(&resp, writer)?;
    let conv_id = resp.conversation_id;

    let stdin = std::io::stdin();
    let reader = stdin.lock();
    interactive_loop(client, writer, reader, conv_id).await
}

async fn interactive_loop(
    client: &impl ApiClient,
    writer: &mut impl Write,
    mut reader: impl BufRead,
    conversation_id: Uuid,
) -> anyhow::Result<()> {
    loop {
        write!(writer, "\n> ")?;
        writer.flush()?;

        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 || line.trim().is_empty() {
            break;
        }

        let req = ChatRequest {
            query: line.trim().to_string(),
            collection: None,
            conversation_id: Some(conversation_id),
            language: None,
            history_limit: None,
        };

        let resp = client.chat(&req).await?;
        format_chat(&resp, writer)?;
    }
    Ok(())
}

fn format_chat<W: Write>(resp: &ChatResponse, w: &mut W) -> anyhow::Result<()> {
    writeln!(w, "{}", resp.answer)?;

    if !resp.citations.is_empty() {
        writeln!(w)?;
        for (i, c) in resp.citations.iter().enumerate() {
            writeln!(
                w,
                "  [{}] {} ({}, chunk {})",
                i + 1,
                c.chunk_id,
                c.document_id,
                c.chunk_index
            )?;
        }
    }

    writeln!(w)?;
    writeln!(
        w,
        "[conversation: {}] [model: {}]",
        resp.conversation_id, resp.model
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::sync::Mutex;

    struct FakeChatClient {
        calls: Mutex<Vec<ChatRequest>>,
    }

    impl FakeChatClient {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl rag_client::ApiClient for FakeChatClient {
        async fn health(&self) -> Result<(), ClientError> {
            Ok(())
        }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> {
            unimplemented!()
        }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> {
            unimplemented!()
        }
        async fn search_dense(
            &self,
            _: &SearchRequest,
        ) -> Result<SearchResponse, ClientError> {
            unimplemented!()
        }
        async fn search_sparse(
            &self,
            _: &SearchRequest,
        ) -> Result<SearchResponse, ClientError> {
            unimplemented!()
        }
        async fn search_hybrid(
            &self,
            _: &HybridSearchRequest,
        ) -> Result<HybridSearchResponse, ClientError> {
            unimplemented!()
        }
        #[allow(clippy::disallowed_methods)]
        async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError> {
            self.calls.lock().unwrap().push(ChatRequest {
                query: req.query.clone(),
                collection: req.collection.clone(),
                conversation_id: req.conversation_id,
                language: None,
                history_limit: None,
            });
            Ok(ChatResponse {
                answer: "test answer".into(),
                conversation_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
                    .unwrap(),
                citations: vec![Citation {
                    chunk_id: "c1".into(),
                    document_id: "d1".into(),
                    chunk_index: 0,
                    sources: vec![],
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
                model: "mock".into(),
            })
        }
        async fn collection_stats(
            &self,
            _: &str,
        ) -> Result<CollectionStatsResponse, ClientError> {
            unimplemented!()
        }
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn chat_single_shot() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        run(
            &client,
            &mut buf,
            false,
            "hello".into(),
            Some("coll".into()),
            false,
            None,
        )
        .await
        .unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("test answer"));
        assert!(output.contains("[1] c1 (d1, chunk 0)"));
        assert!(output.contains("[conversation: 550e8400"));
        assert!(output.contains("[model: mock]"));
    }

    #[tokio::test]
    async fn chat_requires_collection_without_conversation_id() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        let result =
            run(&client, &mut buf, false, "hello".into(), None, false, None).await;
        assert!(result.is_err());
        assert!(result
            .expect_err("should fail")
            .to_string()
            .contains("--collection is required"));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn chat_allows_no_collection_with_conversation_id() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        let conv_id =
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let result =
            run(&client, &mut buf, false, "hello".into(), None, false, Some(conv_id)).await;
        assert!(result.is_ok());
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn chat_conversation_id_threaded() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();

        // First call
        run(
            &client,
            &mut buf,
            false,
            "first".into(),
            Some("coll".into()),
            false,
            None,
        )
        .await
        .unwrap();

        // Simulate second call with conversation_id from first response
        let conv_id =
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        run(
            &client,
            &mut buf,
            false,
            "second".into(),
            None,
            false,
            Some(conv_id),
        )
        .await
        .unwrap();

        let calls = client.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].collection.is_some());
        assert!(calls[1].collection.is_none());
        assert_eq!(calls[1].conversation_id, Some(conv_id));
    }

    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn chat_json_output() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        run(
            &client,
            &mut buf,
            true,
            "hello".into(),
            Some("coll".into()),
            false,
            None,
        )
        .await
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["answer"], "test answer");
        assert!(parsed["conversation_id"].is_string());
    }
}
