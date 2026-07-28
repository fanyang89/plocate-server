use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::dto::{TrendingItem, TrendingResponse};
use crate::error::{AppError, Result};
use crate::limits::validate_query;
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct TrendingParams {
    /// Maximum number of items to return. Defaults to the server-configured
    /// `--trending-top-n`; clamped to 1..=100.
    pub limit: Option<usize>,
}

/// Top searches within the rolling window. Empty when `--trending-enabled=false`.
#[utoipa::path(
    get,
    path = "/api/trending",
    tag = "lifecycle",
    params(TrendingParams),
    responses((status = 200, description = "Trending queries", body = TrendingResponse))
)]
pub async fn trending(
    State(state): State<AppState>,
    Query(params): Query<TrendingParams>,
) -> Result<Json<TrendingResponse>> {
    let (window_secs, items) = match &state.trending {
        Some(t) => {
            let limit = params
                .limit
                .unwrap_or_else(|| t.default_top_n())
                .clamp(1, crate::trending::MAX_TOP_N);
            (t.window_secs(), t.top_k(limit))
        }
        None => (0, Vec::new()),
    };
    Ok(Json(TrendingResponse {
        window_secs,
        items: items
            .into_iter()
            .map(|(query, count)| TrendingItem { query, count })
            .collect(),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DeleteTrendingParams {
    /// Query to permanently exclude from the trending board.
    pub q: String,
}

/// Permanently exclude a query from the trending board.
#[utoipa::path(
    delete,
    path = "/api/trending",
    tag = "lifecycle",
    params(DeleteTrendingParams),
    responses(
        (status = 204, description = "Query blocked from trending"),
        (status = 400, description = "Bad request", body = serde_json::Value),
        (status = 404, description = "Trending is disabled", body = serde_json::Value),
        (status = 500, description = "Blocklist write failed", body = serde_json::Value),
    )
)]
pub async fn delete_trending(
    State(state): State<AppState>,
    Query(params): Query<DeleteTrendingParams>,
) -> Result<StatusCode> {
    validate_query(&params.q)?;
    if params.q.trim().is_empty() {
        return Err(AppError::BadRequest("query must not be empty".to_string()));
    }
    let tracker = state
        .trending
        .ok_or_else(|| AppError::NotFound("trending is disabled".to_string()))?;
    tokio::task::spawn_blocking(move || tracker.block(&params.q))
        .await
        .map_err(|e| AppError::Internal(format!("blocklist task failed: {e}")))??;
    Ok(StatusCode::NO_CONTENT)
}
