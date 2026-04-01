use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use rag_core::ingest::{IngestBatchOutcome, IngestDirectoryRequest, IngestFileRequest};
use serde::{Deserialize, Serialize};

use crate::state::{ApiError, AppState, Ctx, ErrorBody};

fn sanitize_ingest_error(path: &std::path::Path, error: anyhow::Error) -> String {
    tracing::error!(path = %path.display(), error = format!("{error:#}"), "document ingestion failed");
    "ingestion failed".to_string()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct IngestPathsRequest {
    pub paths: Vec<String>,
    pub collection: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureEntry>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct FailureEntry {
    pub path: String,
    pub error: String,
}

#[utoipa::path(post, path = "/ingest", tag = "Ingest",
    request_body = IngestPathsRequest,
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    responses(
        (status = 200, description = "Ingest results", body = IngestResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn ingest_paths(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<IngestPathsRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    if payload.paths.is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "paths must not be empty".into(),
        });
    }
    let paths: Vec<std::path::PathBuf> =
        payload.paths.iter().map(std::path::PathBuf::from).collect();

    let mut total =
        IngestBatchOutcome { documents: 0, chunks: 0, skipped: 0, failures: Vec::new() };

    for path in &paths {
        if path.is_dir() {
            let req = IngestDirectoryRequest {
                path: path.clone(),
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
                dry_run: payload.dry_run,
            };
            match state.ingest.ingest_directory(req).await {
                Ok(outcome) => {
                    total.documents += outcome.documents;
                    total.chunks += outcome.chunks;
                    total.skipped += outcome.skipped;
                    total.failures.extend(outcome.failures);
                }
                Err(e) => {
                    total.failures.push(rag_core::ingest::DocumentFailure {
                        path: path.clone(),
                        error: sanitize_ingest_error(path, e),
                    });
                }
            }
        } else {
            let req = IngestFileRequest {
                path: path.clone(),
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
                dry_run: payload.dry_run,
            };
            match state.ingest.ingest_file(req).await {
                Ok(outcome) => {
                    total.documents += 1;
                    total.chunks += outcome.chunks_created;
                    if outcome.skipped {
                        total.skipped += 1;
                    }
                }
                Err(e) => {
                    total.failures.push(rag_core::ingest::DocumentFailure {
                        path: path.clone(),
                        error: sanitize_ingest_error(path, e),
                    });
                }
            }
        }
    }

    let failures = total
        .failures
        .into_iter()
        .map(|f| FailureEntry { path: f.path.display().to_string(), error: f.error })
        .collect();

    Ok(Json(IngestResponse {
        documents: total.documents,
        chunks: total.chunks,
        skipped: total.skipped,
        failures,
    }))
}

#[utoipa::path(post, path = "/ingest/upload", tag = "Ingest",
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    request_body(content_type = "multipart/form-data", content = String, description = "File upload with optional 'collection' field"),
    responses(
        (status = 200, description = "Upload ingest results", body = IngestResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn ingest_upload(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, ApiError> {
    let mut file_data: Option<(String, Vec<u8>)> = None;
    let mut collection: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: format!("multipart error: {e}"),
    })? {
        match field.name() {
            Some("file") => {
                let filename = field.file_name().unwrap_or("upload").to_string();
                let bytes = field.bytes().await.map_err(|e| ApiError {
                    status: StatusCode::BAD_REQUEST,
                    message: format!("failed to read file: {e}"),
                })?;
                file_data = Some((filename, bytes.to_vec()));
            }
            Some("collection") => {
                let text = field.text().await.map_err(|e| ApiError {
                    status: StatusCode::BAD_REQUEST,
                    message: format!("failed to read collection field: {e}"),
                })?;
                if !text.is_empty() {
                    collection = Some(text);
                }
            }
            _ => {} // ignore unknown fields
        }
    }

    let (filename, bytes) = file_data.ok_or_else(|| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: "missing 'file' field in multipart body".into(),
    })?;

    let extension =
        std::path::Path::new(&filename).extension().and_then(|e| e.to_str()).unwrap_or("bin");

    let tmp = tempfile::NamedTempFile::with_suffix(format!(".{extension}"))
        .map_err(|e| ApiError::internal_with_context("failed to create temp file", e))?;

    tokio::fs::write(tmp.path(), &bytes)
        .await
        .map_err(|e| ApiError::internal_with_context("failed to write temp file", e))?;

    let req = IngestFileRequest {
        path: tmp.path().to_path_buf(),
        tenant: ctx.tenant,
        collection_override: collection,
        dry_run: false,
    };

    let outcome = state
        .ingest
        .ingest_file_trusted(req)
        .await
        .map_err(|e| ApiError::internal_with_context("file upload ingestion failed", e))?;

    Ok(Json(IngestResponse {
        documents: 1,
        chunks: outcome.chunks_created,
        skipped: if outcome.skipped { 1 } else { 0 },
        failures: Vec::new(),
    }))
}
