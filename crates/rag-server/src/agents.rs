use std::collections::HashMap;
use std::sync::Arc;

use agent_core::ports::{ApprovalPort, BaselineAnswerPort, ChatPort, RetrievalPort};
use agent_core::runtime::AgentRuntime;
use agent_core::runtime::graph_flow::GraphFlowRuntime;
use agent_core::spec::{AgentSpec, DefaultToolRegistry};
use agent_core::types::{
    AgentRunConfig, AgentRunResult, CheckpointDecision, GroundedAnswer as AgentGroundedAnswer,
    PendingCheckpoint, RunId, ScoredChunk,
};
use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use rag_core::{ChatService, FusedChunk, RetrievalService, RetrievedChunk, Stores};

#[derive(Clone)]
pub struct AgentManager {
    registry: agent_core::AgentRegistry,
    runtimes: HashMap<String, Arc<dyn AgentRuntime>>,
    runs: Arc<DashMap<RunId, RunRecord>>,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: RunId,
    pub agent_id: String,
    pub query: String,
    pub collection: String,
    pub tenant: String,
}

#[derive(Debug, Clone)]
pub struct AgentDescriptor {
    pub agent_id: String,
    pub description: String,
    pub spec_version: String,
    pub tasks: Vec<String>,
    pub checkpoint_count: usize,
}

#[derive(Debug, Clone)]
pub struct RunWithMetadata {
    pub record: RunRecord,
    pub result: AgentRunResult,
}

impl AgentManager {
    pub async fn load_default(
        agent_specs_dir: &std::path::Path,
        stores: Stores,
        retrieval: Arc<RetrievalService>,
        chat: Arc<ChatService>,
    ) -> Result<Self> {
        let tool_registry = DefaultToolRegistry;
        let registry = agent_core::AgentRegistry::load_from_dir(
            agent_specs_dir,
            &HashMap::new(),
            &tool_registry,
        )
        .await
        .with_context(|| format!("loading agent specs from {}", agent_specs_dir.display()))?;

        let mut runtimes: HashMap<String, Arc<dyn AgentRuntime>> = HashMap::new();
        for (agent_id, spec) in registry.list() {
            let runtime: Arc<dyn AgentRuntime> =
                Arc::new(GraphFlowRuntime::from_spec_with_baseline(
                    spec.clone(),
                    Arc::new(ServerRetrievalPort {
                        retrieval: retrieval.clone(),
                        stores: stores.clone(),
                    }),
                    Arc::new(ServerChatPort { chat: chat.clone() }),
                    Arc::new(PauseForApproval),
                    Arc::new(ServerBaselineAnswerPort { chat: chat.clone() }),
                ));
            runtimes.insert(agent_id.clone(), runtime);
        }

        Ok(Self { registry, runtimes, runs: Arc::new(DashMap::new()) })
    }

    pub fn list_agents(&self) -> Vec<AgentDescriptor> {
        let mut agents: Vec<_> = self.registry.list().values().map(describe_agent).collect();
        agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        agents
    }

    pub fn get_agent(&self, agent_id: &str) -> Option<AgentDescriptor> {
        self.registry.get(agent_id).map(describe_agent)
    }

    pub async fn execute(
        &self,
        agent_id: &str,
        query: String,
        collection: String,
        tenant: String,
        max_steps: usize,
    ) -> Result<AgentRunResult> {
        let runtime = self
            .runtimes
            .get(agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown agent '{agent_id}'"))?;
        let result = runtime
            .start(AgentRunConfig {
                query: query.clone(),
                collection: collection.clone(),
                tenant: tenant.clone(),
                max_steps,
            })
            .await
            .with_context(|| format!("starting agent run for '{agent_id}'"))?;
        self.runs.insert(
            result.run_id,
            RunRecord {
                run_id: result.run_id,
                agent_id: agent_id.into(),
                query,
                collection,
                tenant,
            },
        );
        Ok(result)
    }

    pub async fn inspect(&self, run_id: RunId) -> Result<Option<RunWithMetadata>> {
        let record = match self.runs.get(&run_id) {
            Some(record) => record.clone(),
            None => return Ok(None),
        };
        let runtime = self
            .runtimes
            .get(&record.agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("runtime for agent '{}' is unavailable", record.agent_id))?;
        let result = runtime.inspect(run_id).await?;
        Ok(result.map(|result| RunWithMetadata { record, result }))
    }

    pub async fn inspect_for_tenant(
        &self,
        run_id: RunId,
        tenant: &str,
    ) -> Result<Option<RunWithMetadata>> {
        match self.inspect(run_id).await? {
            Some(run) if run.record.tenant == tenant => Ok(Some(run)),
            Some(_) => Ok(None),
            None => Ok(None),
        }
    }

    pub async fn list_runs(
        &self,
        tenant: &str,
        agent_id: Option<&str>,
    ) -> Result<Vec<RunWithMetadata>> {
        let records: Vec<RunRecord> = self
            .runs
            .iter()
            .filter_map(|entry| {
                let record = entry.value().clone();
                if record.tenant == tenant
                    && agent_id.is_none_or(|filter| filter == record.agent_id)
                {
                    Some(record)
                } else {
                    None
                }
            })
            .collect();

        let mut runs = Vec::new();
        for record in records {
            if let Some(run) = self.inspect(record.run_id).await? {
                runs.push(run);
            }
        }
        runs.sort_by(|a, b| a.record.run_id.cmp(&b.record.run_id));
        Ok(runs)
    }

    pub async fn decide(
        &self,
        run_id: RunId,
        tenant: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<AgentRunResult> {
        let record = self
            .runs
            .get(&run_id)
            .map(|entry| entry.clone())
            .ok_or_else(|| anyhow!("unknown run '{run_id}'"))?;
        if record.tenant != tenant {
            return Err(anyhow!("unknown run '{run_id}'"));
        }
        let runtime = self
            .runtimes
            .get(&record.agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("runtime for agent '{}' is unavailable", record.agent_id))?;
        runtime.resume(run_id, CheckpointDecision { approved, reason }).await
    }
}

fn describe_agent(spec: &AgentSpec) -> AgentDescriptor {
    AgentDescriptor {
        agent_id: spec.agent_id.clone(),
        description: spec.description.clone(),
        spec_version: spec.spec_version.clone(),
        tasks: spec.graph.tasks.clone(),
        checkpoint_count: spec.checkpoints.len(),
    }
}

struct ServerRetrievalPort {
    retrieval: Arc<RetrievalService>,
    stores: Stores,
}

fn map_retrieved_chunk(chunk: RetrievedChunk) -> ScoredChunk {
    ScoredChunk {
        chunk_id: chunk.chunk_id,
        document_id: chunk.document_id,
        chunk_index: chunk.chunk_index,
        text: chunk.text,
        title: chunk.title,
        source_url: chunk.source_url,
        source_domain: chunk.source_domain,
        language: chunk.language,
        tags: chunk.tags,
        section_heading: chunk.section_heading,
        collection: chunk.collection,
        score: chunk.score,
        score_type: chunk.score_type,
        sources: Vec::new(),
        source_scores: std::collections::HashMap::new(),
    }
}

fn map_fused_chunk(chunk: FusedChunk) -> ScoredChunk {
    ScoredChunk {
        chunk_id: chunk.chunk_id,
        document_id: chunk.document_id,
        chunk_index: chunk.chunk_index,
        text: chunk.text,
        title: chunk.title,
        source_url: chunk.source_url,
        source_domain: chunk.source_domain,
        language: chunk.language,
        tags: chunk.tags,
        section_heading: chunk.section_heading,
        collection: chunk.collection,
        score: chunk.fused_score,
        score_type: chunk.score_type,
        sources: chunk.sources,
        source_scores: chunk.source_scores,
    }
}

#[async_trait::async_trait]
impl RetrievalPort for ServerRetrievalPort {
    async fn search_dense(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<ScoredChunk>> {
        let chunks = self.retrieval.search_dense(collection, query, tenant, limit).await?;
        Ok(chunks.into_iter().map(map_retrieved_chunk).collect())
    }

    async fn search_sparse(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<ScoredChunk>> {
        let chunks = self.retrieval.search_sparse(collection, query, tenant, limit).await?;
        Ok(chunks.into_iter().map(map_retrieved_chunk).collect())
    }

    async fn search_hybrid(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
    ) -> Result<Vec<ScoredChunk>> {
        let chunks = self.retrieval.search_hybrid(collection, query, tenant, None).await?;
        Ok(chunks.into_iter().map(map_fused_chunk).collect())
    }

    async fn search_fts(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<ScoredChunk>> {
        let chunks = self.retrieval.search_fts(collection, query, tenant, limit).await?;
        Ok(chunks.into_iter().map(map_retrieved_chunk).collect())
    }

    async fn expand_chunk_neighbors(
        &self,
        tenant: &str,
        document_id: &str,
        chunk_index: i32,
        before: i32,
        after: i32,
    ) -> Result<Vec<ScoredChunk>> {
        let chunks = self
            .retrieval
            .expand_chunk_neighbors(tenant, document_id, chunk_index, before, after)
            .await?;
        Ok(chunks.into_iter().map(map_retrieved_chunk).collect())
    }

    async fn fetch_document(&self, tenant: &str, document_id: &str) -> Result<serde_json::Value> {
        let document = self.stores.get_document(tenant, document_id).await?.ok_or_else(|| {
            anyhow!("document '{document_id}' does not exist for tenant '{tenant}'")
        })?;
        let chunks = self.stores.get_chunks_by_document(tenant, document_id).await?;

        Ok(serde_json::json!({
            "tenant": document.tenant,
            "document": {
                "id": document.id,
                "title": document.title,
                "language": document.language,
                "metadata": document.metadata,
                "source_path": document.source_path,
                "version": document.version,
                "checksum": document.checksum,
                "ingest_run_id": document.ingest_run_id,
                "token_count": document.token_count,
                "collection": document.collection,
                "stats_collection": document.stats_collection,
                "stats_token_count": document.stats_token_count,
                "created_at": document.created_at,
                "updated_at": document.updated_at,
            },
            "chunks": chunks.into_iter().map(|chunk| serde_json::json!({
                "id": chunk.id,
                "tenant": chunk.tenant,
                "document_id": chunk.document_id,
                "chunk_index": chunk.chunk_index,
                "text": chunk.text,
            })).collect::<Vec<_>>(),
        }))
    }
}

struct ServerChatPort {
    chat: Arc<ChatService>,
}

struct ServerBaselineAnswerPort {
    chat: Arc<ChatService>,
}

#[async_trait::async_trait]
impl ChatPort for ServerChatPort {
    async fn summarize(
        &self,
        query: &str,
        chunks: &[ScoredChunk],
        _tenant: &str,
    ) -> Result<String> {
        // Summarization operates only on already-retrieved in-memory chunks.
        // Tenant isolation is enforced at retrieval time before these chunks
        // are passed into agent-core.
        let fused_chunks = chunks
            .iter()
            .map(|chunk| FusedChunk {
                chunk_id: chunk.chunk_id.clone(),
                document_id: chunk.document_id.clone(),
                chunk_index: chunk.chunk_index,
                text: chunk.text.clone(),
                title: None,
                source_url: None,
                source_domain: None,
                language: None,
                tags: Vec::new(),
                section_heading: None,
                collection: None,
                fused_score: chunk.score,
                score_type: "agent_summary".to_string(),
                sources: vec!["agent-core".to_string()],
                source_scores: std::collections::HashMap::new(),
            })
            .collect();
        self.chat.summarize_chunks(query, fused_chunks).await
    }
}

#[async_trait::async_trait]
impl BaselineAnswerPort for ServerBaselineAnswerPort {
    async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> Result<AgentGroundedAnswer> {
        let grounded = self.chat.answer_single_shot(query, collection, tenant, language).await?;

        Ok(AgentGroundedAnswer {
            answer: grounded.answer,
            search_results: grounded.evidence.into_iter().map(map_fused_chunk).collect(),
            citations: grounded.citations.into_iter().map(|citation| citation.chunk_id).collect(),
            model: grounded.model,
        })
    }
}

struct PauseForApproval;

#[async_trait::async_trait]
impl ApprovalPort for PauseForApproval {
    async fn request_approval(
        &self,
        _checkpoint: &PendingCheckpoint,
    ) -> Result<Option<CheckpointDecision>> {
        Ok(None)
    }
}
