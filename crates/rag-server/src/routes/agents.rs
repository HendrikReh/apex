use std::sync::Arc;

use agent_core::types::{AgentRunResult, PendingCheckpoint, QueryType, ScoredChunk, StepRecord};
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
    pub pending_checkpoint: Option<PendingCheckpointResponse>,
    pub steps: Vec<StepRecordResponse>,
    pub search_results: Option<Vec<ScoredChunkResponse>>,
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
    pub score: f32,
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
            ctx.tenant.as_str().to_string(),
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
    _ctx: Ctx,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<RunSummaryResponse>>, ApiError> {
    let runs = state
        .agents
        .list_runs(query.agent_id.as_deref())
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(|run| RunSummaryResponse {
            run_id: run.record.run_id,
            agent_id: run.record.agent_id,
            state: run.result.state.to_string(),
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
    _ctx: Ctx,
    Path(run_id): Path<Uuid>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let run = state.agents.inspect(run_id).await.map_err(ApiError::from)?.ok_or_else(|| {
        ApiError { status: StatusCode::NOT_FOUND, message: format!("run '{run_id}' not found") }
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
    _ctx: Ctx,
    Path(run_id): Path<Uuid>,
    Json(payload): Json<CheckpointDecisionRequest>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let result =
        state.agents.decide(run_id, true, payload.reason).await.map_err(classify_run_error)?;
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
    _ctx: Ctx,
    Path(run_id): Path<Uuid>,
    Json(payload): Json<CheckpointDecisionRequest>,
) -> Result<Json<AgentRunResponse>, ApiError> {
    let result =
        state.agents.decide(run_id, false, payload.reason).await.map_err(classify_run_error)?;
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
        state: result.state.to_string(),
        answer: result.answer,
        summary: result.summary,
        query_type: result.query_type.map(map_query_type),
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

fn map_step_record(step: StepRecord) -> StepRecordResponse {
    StepRecordResponse {
        task_id: step.task_id,
        started_at: step.started_at.to_rfc3339(),
        elapsed_ms: step.elapsed_ms,
        status: format!("{:?}", step.status).to_ascii_lowercase(),
    }
}

fn map_scored_chunk(chunk: ScoredChunk) -> ScoredChunkResponse {
    ScoredChunkResponse {
        chunk_id: chunk.chunk_id,
        document_id: chunk.document_id,
        chunk_index: chunk.chunk_index,
        text: chunk.text,
        score: chunk.score,
    }
}

trait AgentStateString {
    fn to_string(self) -> String;
}

impl AgentStateString for agent_core::types::AgentState {
    fn to_string(self) -> String {
        match self {
            agent_core::types::AgentState::Running => "running".into(),
            agent_core::types::AgentState::AwaitingApproval => "awaiting_approval".into(),
            agent_core::types::AgentState::Completed => "completed".into(),
            agent_core::types::AgentState::Failed => "failed".into(),
        }
    }
}
