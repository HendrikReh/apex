use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    run_registry_policy: RunRegistryPolicy,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: RunId,
    pub agent_id: String,
    pub query: String,
    pub collection: String,
    pub tenant: String,
    inserted_at: Instant,
    awaiting_approval: bool,
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

#[derive(Debug, Clone, Copy)]
struct RunRegistryPolicy {
    ttl: Duration,
    max_entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TenantInspectDisposition {
    NotOwned,
    ProtectRun,
    Unprotected,
}

const DEFAULT_RUN_REGISTRY_TTL_SECS: u64 = 60 * 60 * 24;
const DEFAULT_RUN_REGISTRY_MAX_ENTRIES: usize = 10_000;
const RUN_REGISTRY_TTL_ENV: &str = "AGENT_RUN_REGISTRY_TTL_SECS";
const RUN_REGISTRY_MAX_ENTRIES_ENV: &str = "AGENT_RUN_REGISTRY_MAX_ENTRIES";

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
            let runtime: Arc<dyn AgentRuntime> = Arc::new(GraphFlowRuntime::from_spec(
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

        Ok(Self {
            registry,
            runtimes,
            runs: Arc::new(DashMap::new()),
            run_registry_policy: load_run_registry_policy()?,
        })
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
        self.prune_run_registry();
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
                inserted_at: Instant::now(),
                awaiting_approval: result.pending_checkpoint.is_some(),
            },
        );
        self.prune_run_registry();
        Ok(result)
    }

    pub async fn inspect(&self, run_id: RunId) -> Result<Option<RunWithMetadata>> {
        self.inspect_inner(run_id, Some(run_id)).await
    }

    async fn inspect_inner(
        &self,
        run_id: RunId,
        protected_run_id: Option<RunId>,
    ) -> Result<Option<RunWithMetadata>> {
        if let Some(protected_run_id) = protected_run_id {
            self.prune_run_registry_with_protected(Some(protected_run_id));
        }
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
        let disposition = match self.runs.get(&run_id) {
            Some(record) => tenant_inspect_disposition(record.value(), tenant),
            None => return Ok(None),
        };

        match disposition {
            TenantInspectDisposition::NotOwned => {
                // Do not protect foreign runs from pruning during unauthorized probes.
                self.prune_run_registry();
                Ok(None)
            }
            TenantInspectDisposition::ProtectRun => self.inspect_inner(run_id, Some(run_id)).await,
            TenantInspectDisposition::Unprotected => {
                // Enforce TTL/cap for tenant-owned runs unless they are still awaiting approval.
                self.prune_run_registry();
                self.inspect_inner(run_id, None).await
            }
        }
    }

    pub async fn list_runs(
        &self,
        tenant: &str,
        agent_id: Option<&str>,
    ) -> Result<Vec<RunWithMetadata>> {
        self.prune_run_registry();
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
            if let Some(run) = self.inspect_inner(record.run_id, None).await? {
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
        self.prune_run_registry_with_protected(Some(run_id));
        let runtime = self
            .runtimes
            .get(&record.agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("runtime for agent '{}' is unavailable", record.agent_id))?;
        let result = runtime.resume(run_id, CheckpointDecision { approved, reason }).await?;
        if let Some(mut entry) = self.runs.get_mut(&run_id) {
            entry.awaiting_approval = result.pending_checkpoint.is_some();
            entry.inserted_at = Instant::now();
        }
        Ok(result)
    }

    fn prune_run_registry(&self) {
        self.prune_run_registry_with_protected(None);
    }

    fn prune_run_registry_with_protected(&self, protected_run_id: Option<RunId>) {
        let now = Instant::now();
        let expired =
            prune_expired_runs(&self.runs, now, self.run_registry_policy.ttl, protected_run_id);
        let overflow = prune_to_max_entries(
            &self.runs,
            self.run_registry_policy.max_entries,
            protected_run_id,
        );
        if expired > 0 || overflow > 0 {
            tracing::debug!(
                expired,
                overflow,
                max_entries = self.run_registry_policy.max_entries,
                ttl_secs = self.run_registry_policy.ttl.as_secs(),
                remaining = self.runs.len(),
                "pruned in-memory agent run registry"
            );
        }
    }
}

fn load_run_registry_policy() -> Result<RunRegistryPolicy> {
    fn parse_env<T>(name: &str) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        <T as std::str::FromStr>::Err: std::error::Error + Send + Sync + 'static,
    {
        std::env::var(name)
            .ok()
            .map(|raw| raw.parse::<T>().with_context(|| format!("parsing {name}")))
            .transpose()
    }

    let ttl_secs: u64 = parse_env(RUN_REGISTRY_TTL_ENV)?.unwrap_or(DEFAULT_RUN_REGISTRY_TTL_SECS);
    if ttl_secs == 0 {
        anyhow::bail!("{RUN_REGISTRY_TTL_ENV} must be greater than zero");
    }

    let max_entries: usize =
        parse_env(RUN_REGISTRY_MAX_ENTRIES_ENV)?.unwrap_or(DEFAULT_RUN_REGISTRY_MAX_ENTRIES);
    if max_entries == 0 {
        anyhow::bail!("{RUN_REGISTRY_MAX_ENTRIES_ENV} must be greater than zero");
    }

    Ok(RunRegistryPolicy { ttl: Duration::from_secs(ttl_secs), max_entries })
}

fn prune_expired_runs(
    runs: &DashMap<RunId, RunRecord>,
    now: Instant,
    ttl: Duration,
    protected_run_id: Option<RunId>,
) -> usize {
    let stale_run_ids: Vec<RunId> = runs
        .iter()
        .filter_map(|entry| {
            if protected_run_id.is_some_and(|run_id| run_id == *entry.key()) {
                return None;
            }
            if entry.value().awaiting_approval {
                return None;
            }
            let age = now.saturating_duration_since(entry.value().inserted_at);
            if age >= ttl { Some(*entry.key()) } else { None }
        })
        .collect();
    let removed = stale_run_ids.len();
    for run_id in stale_run_ids {
        runs.remove(&run_id);
    }
    removed
}

fn prune_to_max_entries(
    runs: &DashMap<RunId, RunRecord>,
    max_entries: usize,
    protected_run_id: Option<RunId>,
) -> usize {
    let current_len = runs.len();
    if current_len <= max_entries {
        return 0;
    }

    let overflow = current_len - max_entries;
    let mut eviction_candidates: Vec<(RunId, bool, Instant)> = runs
        .iter()
        .filter_map(|entry| {
            if protected_run_id.is_some_and(|run_id| run_id == *entry.key()) {
                None
            } else {
                Some((*entry.key(), entry.value().awaiting_approval, entry.value().inserted_at))
            }
        })
        .collect();
    eviction_candidates
        .sort_by_key(|(_, awaiting_approval, inserted_at)| (*awaiting_approval, *inserted_at));

    for (run_id, _, _) in eviction_candidates.into_iter().take(overflow) {
        runs.remove(&run_id);
    }
    current_len.saturating_sub(runs.len())
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

fn tenant_inspect_disposition(record: &RunRecord, tenant: &str) -> TenantInspectDisposition {
    if record.tenant != tenant {
        TenantInspectDisposition::NotOwned
    } else if record.awaiting_approval {
        TenantInspectDisposition::ProtectRun
    } else {
        TenantInspectDisposition::Unprotected
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

fn map_scored_chunk_for_summary(chunk: &ScoredChunk) -> FusedChunk {
    FusedChunk {
        chunk_id: chunk.chunk_id.clone(),
        document_id: chunk.document_id.clone(),
        chunk_index: chunk.chunk_index,
        text: chunk.text.clone(),
        title: chunk.title.clone(),
        source_url: chunk.source_url.clone(),
        source_domain: chunk.source_domain.clone(),
        language: chunk.language.clone(),
        tags: chunk.tags.clone(),
        section_heading: chunk.section_heading.clone(),
        collection: chunk.collection.clone(),
        fused_score: chunk.score,
        score_type: chunk.score_type.clone(),
        sources: chunk.sources.clone(),
        source_scores: chunk.source_scores.clone(),
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

    #[allow(clippy::disallowed_methods)] // serde_json::json! internally uses unwrap()
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
        let fused_chunks = chunks.iter().map(map_scored_chunk_for_summary).collect();
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::*;
    use uuid::Uuid;

    fn sample_record(run_id: RunId, inserted_at: Instant, awaiting_approval: bool) -> RunRecord {
        RunRecord {
            run_id,
            agent_id: "rag_spike".to_string(),
            query: "test query".to_string(),
            collection: "test-collection".to_string(),
            tenant: "test-tenant".to_string(),
            inserted_at,
            awaiting_approval,
        }
    }

    #[test]
    fn tenant_inspect_disposition_for_foreign_run_is_not_owned() {
        let record = sample_record(Uuid::new_v4(), Instant::now(), false);

        assert_eq!(
            tenant_inspect_disposition(&record, "other-tenant"),
            TenantInspectDisposition::NotOwned
        );
    }

    #[test]
    fn tenant_inspect_disposition_for_owned_awaiting_run_is_protected() {
        let record = sample_record(Uuid::new_v4(), Instant::now(), true);

        assert_eq!(
            tenant_inspect_disposition(&record, "test-tenant"),
            TenantInspectDisposition::ProtectRun
        );
    }

    #[test]
    fn tenant_inspect_disposition_for_owned_completed_run_is_unprotected() {
        let record = sample_record(Uuid::new_v4(), Instant::now(), false);

        assert_eq!(
            tenant_inspect_disposition(&record, "test-tenant"),
            TenantInspectDisposition::Unprotected
        );
    }

    #[test]
    fn map_scored_chunk_for_summary_preserves_metadata_and_provenance() {
        let chunk = ScoredChunk {
            chunk_id: "chunk-1".to_string(),
            document_id: "doc-1".to_string(),
            chunk_index: 3,
            text: "chunk text".to_string(),
            title: Some("Title".to_string()),
            source_url: Some("https://example.com/doc-1".to_string()),
            source_domain: Some("example.com".to_string()),
            language: Some("en".to_string()),
            tags: vec!["guide".to_string()],
            section_heading: Some("Overview".to_string()),
            collection: Some("docs".to_string()),
            score: 0.75,
            score_type: "fts".to_string(),
            sources: vec!["fts".to_string()],
            source_scores: HashMap::from([("fts".to_string(), 0.75)]),
        };

        let fused = map_scored_chunk_for_summary(&chunk);

        assert_eq!(fused.chunk_id, chunk.chunk_id);
        assert_eq!(fused.document_id, chunk.document_id);
        assert_eq!(fused.chunk_index, chunk.chunk_index);
        assert_eq!(fused.text, chunk.text);
        assert_eq!(fused.title, chunk.title);
        assert_eq!(fused.source_url, chunk.source_url);
        assert_eq!(fused.source_domain, chunk.source_domain);
        assert_eq!(fused.language, chunk.language);
        assert_eq!(fused.tags, chunk.tags);
        assert_eq!(fused.section_heading, chunk.section_heading);
        assert_eq!(fused.collection, chunk.collection);
        assert_eq!(fused.fused_score, chunk.score);
        assert_eq!(fused.score_type, chunk.score_type);
        assert_eq!(fused.sources, chunk.sources);
        assert_eq!(fused.source_scores, chunk.source_scores);
    }

    #[test]
    fn prune_expired_runs_removes_entries_older_than_ttl() {
        let runs = DashMap::new();
        let now = Instant::now();
        let stale_run = Uuid::new_v4();
        let fresh_run = Uuid::new_v4();
        let ttl = Duration::from_secs(30);

        runs.insert(stale_run, sample_record(stale_run, now - Duration::from_secs(120), false));
        runs.insert(fresh_run, sample_record(fresh_run, now - Duration::from_secs(5), false));

        let removed = prune_expired_runs(&runs, now, ttl, None);

        assert_eq!(removed, 1);
        assert!(!runs.contains_key(&stale_run));
        assert!(runs.contains_key(&fresh_run));
    }

    #[test]
    fn prune_to_max_entries_keeps_newest_runs() {
        let runs = DashMap::new();
        let now = Instant::now();
        let oldest = Uuid::new_v4();
        let middle = Uuid::new_v4();
        let newest = Uuid::new_v4();

        runs.insert(oldest, sample_record(oldest, now - Duration::from_secs(30), false));
        runs.insert(middle, sample_record(middle, now - Duration::from_secs(20), false));
        runs.insert(newest, sample_record(newest, now - Duration::from_secs(10), false));

        let removed = prune_to_max_entries(&runs, 2, None);

        assert_eq!(removed, 1);
        assert!(!runs.contains_key(&oldest));
        assert!(runs.contains_key(&middle));
        assert!(runs.contains_key(&newest));
    }

    #[test]
    fn prune_expired_runs_keeps_protected_run() {
        let runs = DashMap::new();
        let now = Instant::now();
        let protected = Uuid::new_v4();
        let stale_unprotected = Uuid::new_v4();
        let ttl = Duration::from_secs(30);

        runs.insert(protected, sample_record(protected, now - Duration::from_secs(120), false));
        runs.insert(
            stale_unprotected,
            sample_record(stale_unprotected, now - Duration::from_secs(120), false),
        );

        let removed = prune_expired_runs(&runs, now, ttl, Some(protected));

        assert_eq!(removed, 1);
        assert!(runs.contains_key(&protected));
        assert!(!runs.contains_key(&stale_unprotected));
    }

    #[test]
    fn prune_to_max_entries_keeps_protected_run() {
        let runs = DashMap::new();
        let now = Instant::now();
        let protected_oldest = Uuid::new_v4();
        let middle = Uuid::new_v4();
        let newest = Uuid::new_v4();

        runs.insert(
            protected_oldest,
            sample_record(protected_oldest, now - Duration::from_secs(30), false),
        );
        runs.insert(middle, sample_record(middle, now - Duration::from_secs(20), false));
        runs.insert(newest, sample_record(newest, now - Duration::from_secs(10), false));

        let removed = prune_to_max_entries(&runs, 2, Some(protected_oldest));

        assert_eq!(removed, 1);
        assert!(runs.contains_key(&protected_oldest));
        assert!(!runs.contains_key(&middle));
        assert!(runs.contains_key(&newest));
    }

    #[test]
    fn prune_expired_runs_keeps_awaiting_approval_run() {
        let runs = DashMap::new();
        let now = Instant::now();
        let awaiting = Uuid::new_v4();
        let stale_unprotected = Uuid::new_v4();
        let ttl = Duration::from_secs(30);

        runs.insert(awaiting, sample_record(awaiting, now - Duration::from_secs(120), true));
        runs.insert(
            stale_unprotected,
            sample_record(stale_unprotected, now - Duration::from_secs(120), false),
        );

        let removed = prune_expired_runs(&runs, now, ttl, None);

        assert_eq!(removed, 1);
        assert!(runs.contains_key(&awaiting));
        assert!(!runs.contains_key(&stale_unprotected));
    }

    #[test]
    fn prune_to_max_entries_keeps_awaiting_approval_run() {
        let runs = DashMap::new();
        let now = Instant::now();
        let awaiting_oldest = Uuid::new_v4();
        let middle = Uuid::new_v4();
        let newest = Uuid::new_v4();

        runs.insert(
            awaiting_oldest,
            sample_record(awaiting_oldest, now - Duration::from_secs(30), true),
        );
        runs.insert(middle, sample_record(middle, now - Duration::from_secs(20), false));
        runs.insert(newest, sample_record(newest, now - Duration::from_secs(10), false));

        let removed = prune_to_max_entries(&runs, 2, None);

        assert_eq!(removed, 1);
        assert!(runs.contains_key(&awaiting_oldest));
        assert!(!runs.contains_key(&middle));
        assert!(runs.contains_key(&newest));
    }

    #[test]
    fn prune_to_max_entries_evicts_awaiting_approval_when_needed_for_cap() {
        let runs = DashMap::new();
        let now = Instant::now();
        let awaiting_oldest = Uuid::new_v4();
        let awaiting_newer = Uuid::new_v4();

        runs.insert(
            awaiting_oldest,
            sample_record(awaiting_oldest, now - Duration::from_secs(30), true),
        );
        runs.insert(
            awaiting_newer,
            sample_record(awaiting_newer, now - Duration::from_secs(10), true),
        );

        let removed = prune_to_max_entries(&runs, 1, None);

        assert_eq!(removed, 1);
        assert!(!runs.contains_key(&awaiting_oldest));
        assert!(runs.contains_key(&awaiting_newer));
    }
}
