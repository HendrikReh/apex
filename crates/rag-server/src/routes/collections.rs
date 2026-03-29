use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::state::{ApiError, AppState, Ctx};

#[derive(Serialize)]
pub struct CollectionStatsResponse {
    pub collection: String,
    pub tenant: String,
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}

pub async fn collection_stats(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Path(collection): Path<String>,
) -> Result<Json<CollectionStatsResponse>, ApiError> {
    let stats = state
        .stores
        .get_corpus_stats(ctx.tenant.as_str(), &collection, f64::from(state.config.bm25_avgdl))
        .await
        .map_err(ApiError::from)?;

    Ok(Json(CollectionStatsResponse {
        collection,
        tenant: ctx.tenant.as_str().to_string(),
        total_docs: stats.total_docs,
        total_tokens: stats.total_tokens,
        avgdl: stats.avgdl,
    }))
}
