use std::sync::Arc;

use agent_core::types::{
    AgentRunResult, PendingCheckpoint, QueryType, RouteDecision, ScoredChunk, StepRecord,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{ApiError, AppState, Ctx, ErrorBody};

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentSummaryResponse {
    pub agent_id: String,
    pub description: String,
    pub spec_version: String,
    pub task_count: usize,
    pub checkpoint_count: usize,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentDetailResponse {
    pub agent_id: String,
    pub description: String,
    pub spec_version: String,
    pub tasks: Vec<String>,
    pub checkpoint_count: usize,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ExecuteAgentRequest {
    pub query: String,
    pub collection: String,
    pub max_steps: Option<usize>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct RunListQuery {
    pub agent_id: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CheckpointDecisionRequest {
    pub reason: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RunSummaryResponse {
    pub run_id: Uuid,
    pub agent_id: String,
    pub state: String,
    pub query: String,
    pub collection: String,
    pub tenant: String,
    pub step_count: usize,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AgentRunResponse {
    pub run_id: Uuid,
    pub state: String,
    pub answer: Option<String>,
    pub summary: Option<String>,
    pub query_type: Option<String>,
    pub route_decision: Option<RouteDecisionResponse>,
    pub pending_checkpoint: Option<PendingCheckpointResponse>,
    pub steps: Vec<StepRecordResponse>,
    pub search_results: Option<Vec<ScoredChunkResponse>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RouteDecisionResponse {
    pub selected_path: String,
    pub query_class: String,
    pub retrieval_profile: String,
    pub ambiguity: bool,
    pub needs_multi_hop: bool,
    pub needs_high_evidence: bool,
    pub time_sensitive: bool,
    pub normalized_filters: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PendingCheckpointResponse {
    pub after_task: String,
    pub summary: Option<String>,
    pub checkpoint_id: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub on_timeout: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct StepRecordResponse {
    pub task_id: String,
    pub started_at: String,
    pub elapsed_ms: u64,
    pub status: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ScoredChunkResponse {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub score: f32,
    pub score_type: String,
    pub sources: Vec<String>,
    pub source_scores: std::collections::HashMap<String, f32>,
}

#[utoipa::path(get, path = "/agents", tag = "Agents",
    responses((status = 200, description = "Available agent specs", body = Vec<AgentSummaryResponse>)),
    security(("api_key" = []))
)]
pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    _ctx: Ctx,
) -> Result<Json<Vec<AgentSummaryResponse>>, ApiError> {
    let agents = state
        .agents
        .list_agents()
        .into_iter()
        .map(|agent| AgentSummaryResponse {
            agent_id: agent.agent_id,
            description: agent.description,
            spec_version: agent.spec_version,
            task_count: agent.tasks.len(),
            checkpoint_count: agent.checkpoint_count,
        })
        .collect();
    Ok(Json(agents))
}

#[utoipa::path(get, path = "/agents/{id}", tag = "Agents",
    params(("id" = String, Path, description = "Agent identifier")),
    responses(
        (status = 200, description = "Agent spec summary", body = AgentDetailResponse),
        (status = 404, description = "Agent not found", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn get_agent(
    State(state): State<Arc<AppState>>,
    _ctx: Ctx,
    Path(agent_id): Path<String>,
) -> Result<Json<AgentDetailResponse>, ApiError> {
    let agent = state.agents.get_agent(&agent_id).ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!("agent '{agent_id}' not found"),
    })?;
    Ok(Json(AgentDetailResponse {
        agent_id: agent.agent_id,
        description: agent.description,
        spec_version: agent.spec_version,
        tasks: agent.tasks,
        checkpoint_count: agent.checkpoint_count,
    }))
}

#[utoipa::path(post, path = "/agents/{id}/execute", tag = "Agents",
    request_body = ExecuteAgentRequest,
    params(
        ("id" = String, Path, description = "Agent identifier"),
        ("x-tenant" = String, Header, description = "Tenant identifier"),
    ),
    responses(
        (status = 200, description = "Agent run started", body = AgentRunResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn execute_agent(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Path(agent_id): Path<String>,
    Json(payload): Json<ExecuteAgentRequest>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    if payload.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if payload.collection.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }
    if payload.max_steps == Some(0) {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "max_steps must be greater than 0".into(),
        });
    }

    let result = state
        .agents
        .execute(
            &agent_id,
            payload.query,
            payload.collection,
            ctx.tenant.to_string(),
            payload.max_steps.unwrap_or(20),
        )
        .await
        .map_err(|error| classify_agent_error(&agent_id, error))?;

    Ok(Json(map_run_response(result)))
}

#[utoipa::path(get, path = "/runs", tag = "Agents",
    params(RunListQuery),
    responses((status = 200, description = "Known agent runs", body = Vec<RunSummaryResponse>)),
    security(("api_key" = []))
)]
pub async fn list_runs(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<RunSummaryResponse>>, ApiError> {
    let runs = state
        .agents
        .list_runs(ctx.tenant.as_str(), query.agent_id.as_deref())
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(|run| RunSummaryResponse {
            run_id: run.record.run_id,
            agent_id: run.record.agent_id,
            state: map_agent_state(run.result.state),
            query: run.record.query,
            collection: run.record.collection,
            tenant: run.record.tenant,
            step_count: run.result.steps.len(),
        })
        .collect();
    Ok(Json(runs))
}

#[utoipa::path(get, path = "/runs/{id}", tag = "Agents",
    params(("id" = Uuid, Path, description = "Run identifier")),
    responses(
        (status = 200, description = "Run state", body = AgentRunResponse),
        (status = 404, description = "Run not found", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn get_run(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Path(run_id): Path<Uuid>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let run = state
        .agents
        .inspect_for_tenant(run_id, ctx.tenant.as_str())
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!("run '{run_id}' not found"),
        })?;
    Ok(Json(map_run_response(run.result)))
}

#[utoipa::path(post, path = "/runs/{id}/approve", tag = "Agents",
    request_body = CheckpointDecisionRequest,
    params(("id" = Uuid, Path, description = "Run identifier")),
    responses(
        (status = 200, description = "Run resumed with approval", body = AgentRunResponse),
        (status = 404, description = "Run not found", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn approve_run(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Path(run_id): Path<Uuid>,
    Json(payload): Json<CheckpointDecisionRequest>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let result = state
        .agents
        .decide(run_id, ctx.tenant.as_str(), true, payload.reason)
        .await
        .map_err(classify_run_error)?;
    Ok(Json(map_run_response(result)))
}

#[utoipa::path(post, path = "/runs/{id}/reject", tag = "Agents",
    request_body = CheckpointDecisionRequest,
    params(("id" = Uuid, Path, description = "Run identifier")),
    responses(
        (status = 200, description = "Run resumed with rejection", body = AgentRunResponse),
        (status = 404, description = "Run not found", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn reject_run(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Path(run_id): Path<Uuid>,
    Json(payload): Json<CheckpointDecisionRequest>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let result = state
        .agents
        .decide(run_id, ctx.tenant.as_str(), false, payload.reason)
        .await
        .map_err(classify_run_error)?;
    Ok(Json(map_run_response(result)))
}

fn classify_agent_error(agent_id: &str, error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("unknown agent") {
        ApiError { status: StatusCode::NOT_FOUND, message: format!("agent '{agent_id}' not found") }
    } else {
        ApiError::internal_with_context("agent execution failed", &error)
    }
}

fn classify_run_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("unknown run") || message.contains("no metadata for run") {
        ApiError { status: StatusCode::NOT_FOUND, message }
    } else if message.contains("AwaitingApproval") || message.contains("cannot resume") {
        ApiError { status: StatusCode::BAD_REQUEST, message }
    } else {
        ApiError::internal_with_context("agent run update failed", &error)
    }
}

fn map_run_response(result: AgentRunResult) -> AgentRunResponse {
    AgentRunResponse {
        run_id: result.run_id,
        state: map_agent_state(result.state),
        answer: result.answer,
        summary: result.summary,
        query_type: result.query_type.map(map_query_type),
        route_decision: result.route_decision.map(map_route_decision),
        pending_checkpoint: result.pending_checkpoint.map(map_pending_checkpoint),
        steps: result.steps.into_iter().map(map_step_record).collect(),
        search_results: result
            .search_results
            .map(|chunks| chunks.into_iter().map(map_scored_chunk).collect()),
    }
}

fn map_query_type(query_type: QueryType) -> String {
    match query_type {
        QueryType::Structured => "structured".into(),
        QueryType::Unstructured => "unstructured".into(),
        QueryType::Mixed => "mixed".into(),
    }
}

fn map_pending_checkpoint(checkpoint: PendingCheckpoint) -> PendingCheckpointResponse {
    PendingCheckpointResponse {
        after_task: checkpoint.after_task,
        summary: checkpoint.summary,
        checkpoint_id: checkpoint.checkpoint_id,
        timeout_seconds: checkpoint.timeout_seconds,
        on_timeout: checkpoint.on_timeout,
    }
}

fn map_route_decision(route_decision: RouteDecision) -> RouteDecisionResponse {
    RouteDecisionResponse {
        selected_path: map_route_path(route_decision.selected_path),
        query_class: map_query_class(route_decision.query_class),
        retrieval_profile: map_retrieval_profile(route_decision.retrieval_profile),
        ambiguity: route_decision.ambiguity,
        needs_multi_hop: route_decision.needs_multi_hop,
        needs_high_evidence: route_decision.needs_high_evidence,
        time_sensitive: route_decision.time_sensitive,
        normalized_filters: route_decision.normalized_filters,
        reasons: route_decision.reasons,
    }
}

fn map_step_record(step: StepRecord) -> StepRecordResponse {
    StepRecordResponse {
        task_id: step.task_id,
        started_at: step.started_at.to_rfc3339(),
        elapsed_ms: step.elapsed_ms,
        status: map_step_status(step.status),
    }
}

fn map_scored_chunk(chunk: ScoredChunk) -> ScoredChunkResponse {
    ScoredChunkResponse {
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
        sources: chunk.sources,
        source_scores: chunk.source_scores,
    }
}

fn map_agent_state(state: agent_core::types::AgentState) -> String {
    match state {
        agent_core::types::AgentState::Running => "running".into(),
        agent_core::types::AgentState::AwaitingApproval => "awaiting_approval".into(),
        agent_core::types::AgentState::Completed => "completed".into(),
        agent_core::types::AgentState::Failed => "failed".into(),
    }
}

fn map_step_status(status: agent_core::types::StepStatus) -> String {
    match status {
        agent_core::types::StepStatus::Completed => "completed".into(),
        agent_core::types::StepStatus::Paused => "paused".into(),
        agent_core::types::StepStatus::Failed => "failed".into(),
    }
}

fn map_route_path(path: agent_core::types::RoutePath) -> String {
    match path {
        agent_core::types::RoutePath::SinglePassRag => "single_pass_rag".into(),
        agent_core::types::RoutePath::AgenticSearch => "agentic_search".into(),
    }
}

fn map_query_class(query_class: agent_core::types::QueryClass) -> String {
    match query_class {
        agent_core::types::QueryClass::SimpleFact => "simple_fact".into(),
        agent_core::types::QueryClass::AmbiguityDisambiguation => "ambiguity_disambiguation".into(),
        agent_core::types::QueryClass::ExploratorySearch => "exploratory_search".into(),
        agent_core::types::QueryClass::Procedural => "procedural".into(),
        agent_core::types::QueryClass::MultiHopResearch => "multi_hop_research".into(),
    }
}

fn map_retrieval_profile(profile: agent_core::types::RetrievalProfileId) -> String {
    match profile {
        agent_core::types::RetrievalProfileId::SimpleHybrid => "simple_hybrid".into(),
        agent_core::types::RetrievalProfileId::LexicalFirst => "lexical_first".into(),
        agent_core::types::RetrievalProfileId::BroadThenExpand => "broad_then_expand".into(),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test assertions use expect()
mod tests {
    use std::collections::HashMap;

    use agent_core::types::{
        AgentState, QueryClass, RetrievalProfileId, RouteDecision, RoutePath, StepStatus,
    };

    use super::*;

    #[test]
    fn map_run_response_preserves_route_decision() {
        let route_decision = RouteDecision {
            selected_path: RoutePath::AgenticSearch,
            query_class: QueryClass::MultiHopResearch,
            retrieval_profile: RetrievalProfileId::BroadThenExpand,
            ambiguity: true,
            needs_multi_hop: true,
            needs_high_evidence: true,
            time_sensitive: false,
            normalized_filters: vec!["tenant:default".into()],
            reasons: vec!["compare".into()],
        };
        let result = AgentRunResult {
            run_id: Uuid::nil(),
            state: AgentState::Completed,
            answer: Some("answer".into()),
            summary: Some("summary".into()),
            search_results: None,
            query_type: Some(QueryType::Unstructured),
            route_decision: Some(route_decision),
            pending_checkpoint: None,
            steps: vec![StepRecord {
                task_id: "final_answer".into(),
                started_at: chrono::Utc::now(),
                elapsed_ms: 1,
                status: StepStatus::Completed,
            }],
        };

        let response = map_run_response(result);

        let route_decision = response.route_decision.expect("route decision");
        assert_eq!(route_decision.selected_path, "agentic_search");
        assert_eq!(route_decision.query_class, "multi_hop_research");
        assert_eq!(route_decision.retrieval_profile, "broad_then_expand");
        assert!(route_decision.ambiguity);
        assert!(route_decision.needs_multi_hop);
        assert_eq!(route_decision.normalized_filters, vec!["tenant:default"]);
        assert_eq!(route_decision.reasons, vec!["compare"]);
    }

    #[test]
    fn map_scored_chunk_preserves_enriched_metadata() {
        let mut source_scores = HashMap::new();
        source_scores.insert("dense".into(), 0.91);
        source_scores.insert("sparse".into(), 0.72);
        let chunk = ScoredChunk {
            chunk_id: "chunk-1".into(),
            document_id: "doc-1".into(),
            chunk_index: 2,
            text: "chunk text".into(),
            title: Some("Doc Title".into()),
            source_url: Some("https://example.com/doc-1".into()),
            source_domain: Some("example.com".into()),
            language: Some("en".into()),
            tags: vec!["guide".into(), "rust".into()],
            section_heading: Some("Overview".into()),
            collection: Some("docs".into()),
            score: 0.88,
            score_type: "rrf_fused".into(),
            sources: vec!["dense".into(), "sparse".into()],
            source_scores,
        };

        let response = map_scored_chunk(chunk);

        assert_eq!(response.title.as_deref(), Some("Doc Title"));
        assert_eq!(response.source_url.as_deref(), Some("https://example.com/doc-1"));
        assert_eq!(response.source_domain.as_deref(), Some("example.com"));
        assert_eq!(response.language.as_deref(), Some("en"));
        assert_eq!(response.tags, vec!["guide", "rust"]);
        assert_eq!(response.section_heading.as_deref(), Some("Overview"));
        assert_eq!(response.collection.as_deref(), Some("docs"));
        assert_eq!(response.score_type, "rrf_fused");
        assert_eq!(response.sources, vec!["dense", "sparse"]);
        assert_eq!(response.source_scores.get("dense"), Some(&0.91));
        assert_eq!(response.source_scores.get("sparse"), Some(&0.72));
    }
}
