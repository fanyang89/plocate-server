use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::dto::SearchResponse;
use crate::error::Result;
use crate::limits::{validate_offset, validate_query};
use crate::state::{AppState, SearchOrigin};

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchParams {
    /// Query string. plocate matches it as a substring (wholename/path) by
    /// default; if it contains glob metacharacters (* ? []) it is treated as
    /// a glob. Multiple patterns are AND-ed.
    pub q: String,
    #[param(default = 100)]
    pub limit: Option<usize>,
    #[param(default = 0)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[param(default = true)]
    pub case: Option<bool>,
    /// `path` (default, match against full path) or `basename`.
    #[param(default = "path")]
    pub scope: Option<String>,
}

/// Filename / path search via the plocate trigram index.
#[utoipa::path(
    get,
    path = "/api/search",
    tag = "search",
    params(SearchParams),
    responses(
        (status = 200, description = "Search results", body = SearchResponse),
        (status = 400, description = "Bad request", body = serde_json::Value),
        (status = 500, description = "Internal error", body = serde_json::Value),
    )
)]
pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>> {
    validate_query(&params.q)?;
    let limit = clamp_limit(params.limit, state.max_results);
    let offset = validate_offset(params.offset)?;
    let case_insensitive = params.case.unwrap_or(true);
    let basename_only = matches!(
        params.scope.as_deref(),
        Some("basename") | Some("b") | Some("name")
    );
    let resp = state
        .search(
            &params.q,
            limit,
            offset,
            case_insensitive,
            basename_only,
            SearchOrigin::Rest,
        )
        .await?;
    Ok(Json(resp))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct GlobParams {
    /// Glob pattern, e.g. `*.rs` or `**/2024/*.log`.
    pub pattern: String,
    #[param(default = 100)]
    pub limit: Option<usize>,
    #[param(default = 0)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[param(default = true)]
    pub case: Option<bool>,
}

/// Glob search. Identical to `/api/search` but explicit about glob intent.
#[utoipa::path(
    get,
    path = "/api/glob",
    tag = "search",
    params(GlobParams),
    responses(
        (status = 200, description = "Glob results", body = SearchResponse),
        (status = 400, description = "Bad request", body = serde_json::Value),
        (status = 500, description = "Internal error", body = serde_json::Value),
    )
)]
pub async fn glob(
    State(state): State<AppState>,
    Query(params): Query<GlobParams>,
) -> Result<Json<SearchResponse>> {
    validate_query(&params.pattern)?;
    let limit = clamp_limit(params.limit, state.max_results);
    let offset = validate_offset(params.offset)?;
    let case_insensitive = params.case.unwrap_or(true);
    let resp = state
        .search(
            &params.pattern,
            limit,
            offset,
            case_insensitive,
            false,
            SearchOrigin::Rest,
        )
        .await?;
    Ok(Json(resp))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FuzzyParams {
    /// Query string. Whitespace-separated tokens are matched with AND
    /// semantics (every token must appear in the path); results are ranked by
    /// fzf-style fuzzy relevance via nucleo.
    pub q: String,
    #[param(default = 100)]
    pub limit: Option<usize>,
    #[param(default = 0)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[param(default = true)]
    pub case: Option<bool>,
}

/// Fuzzy multi-keyword search with relevance ranking.
#[utoipa::path(
    get,
    path = "/api/fuzzy",
    tag = "search",
    params(FuzzyParams),
    responses(
        (status = 200, description = "Ranked search results", body = SearchResponse),
        (status = 400, description = "Bad request", body = serde_json::Value),
        (status = 500, description = "Internal error", body = serde_json::Value),
    )
)]
pub async fn fuzzy(
    State(state): State<AppState>,
    Query(params): Query<FuzzyParams>,
) -> Result<Json<SearchResponse>> {
    validate_query(&params.q)?;
    let limit = clamp_limit(params.limit, state.max_results);
    let offset = validate_offset(params.offset)?;
    let case_insensitive = params.case.unwrap_or(true);
    let resp = state
        .search_fuzzy(
            &params.q,
            limit,
            offset,
            case_insensitive,
            SearchOrigin::Rest,
        )
        .await?;
    Ok(Json(resp))
}

fn clamp_limit(req: Option<usize>, max: usize) -> usize {
    req.unwrap_or(max).clamp(1, max.max(1))
}
