use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use rag_core::ingest::{IngestBatchOutcome, IngestDirectoryRequest, IngestFileRequest};
use serde::{Deserialize, Serialize};

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct IngestPathsRequest {
    pub paths: Vec<String>,
    pub collection: Option<String>,
}

#[derive(Serialize)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureEntry>,
}

#[derive(Serialize)]
pub struct FailureEntry {
    pub path: String,
    pub error: String,
}

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
    // Canonicalize paths (resolves relative paths, symlinks, and verifies existence)
    let mut resolved_paths = Vec::with_capacity(payload.paths.len());
    for p in &payload.paths {
        let path = std::path::PathBuf::from(p);
        let canonical = path.canonicalize().map_err(|e| ApiError {
            status: StatusCode::BAD_REQUEST,
            message: format!("path does not exist or is inaccessible: {p} — {e}"),
        })?;
        resolved_paths.push(canonical);
    }

    let mut total =
        IngestBatchOutcome { documents: 0, chunks: 0, skipped: 0, failures: Vec::new() };

    for path in &resolved_paths {
        if path.is_dir() {
            let req = IngestDirectoryRequest {
                path: path.clone(),
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
                dry_run: false,
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
                        error: format!("{e:#}"),
                    });
                }
            }
        } else {
            let req = IngestFileRequest {
                path: path.clone(),
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
                dry_run: false,
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
                        error: format!("{e:#}"),
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

    let tmp =
        tempfile::NamedTempFile::with_suffix(format!(".{extension}")).map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("failed to create temp file: {e}"),
        })?;

    tokio::fs::write(tmp.path(), &bytes).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to write temp file: {e}"),
    })?;

    let req = IngestFileRequest {
        path: tmp.path().to_path_buf(),
        tenant: ctx.tenant,
        collection_override: collection,
        dry_run: false,
    };

    let outcome = state.ingest.ingest_file(req).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("{e:#}"),
    })?;

    Ok(Json(IngestResponse {
        documents: 1,
        chunks: outcome.chunks_created,
        skipped: if outcome.skipped { 1 } else { 0 },
        failures: Vec::new(),
    }))
}
