pub mod frontend;
pub mod health;
pub mod reindex;
pub mod search;
pub mod stats;
pub mod trending;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use utoipa_swagger_ui::{Config as SwaggerConfig, SwaggerUi};

use crate::mcp::PlocateMcpHandler;
use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Build the production router. When `--public-base-url` is configured the
/// entire surface (REST + Swagger + OpenAPI + MCP + frontend) is nested under
/// the path prefix; otherwise the server is mounted at `/` (legacy behavior).
pub fn router(state: AppState) -> Router {
    let prefix = state.base_url_prefix.as_str().to_owned();
    // Prefer the full canonical URL for OpenAPI `servers` (so Swagger "Try it
    // out" hits the public origin); fall back to the bare prefix path.
    let openapi_server = state
        .base_url_public
        .as_deref()
        .map(|s| s.to_string())
        .or_else(|| {
            let p = state.base_url_prefix.as_str();
            if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            }
        });

    // `SwaggerUi::url()` both registers the in-process JSON route and seeds
    // the browser config. Keep the registered route relative to `inner` so
    // axum's outer `nest(prefix, inner)` exposes it at `{prefix}/openapi.json`;
    // override the browser-facing config with the public prefixed path.
    let openapi_json_path = if prefix.is_empty() {
        "/openapi.json".to_string()
    } else {
        format!("{prefix}/openapi.json")
    };

    // Inner router carries every API/Swagger/MCP route but NO fallback. In
    // axum 0.7 a nested router's fallback does not fire for unmatched nested
    // paths (the outer router's fallback does), so we install the SPA
    // fallback on the outer router regardless of mount mode and let it strip
    // the prefix itself.
    let inner = Router::new()
        .route("/api/search", get(search::search))
        .route("/api/glob", get(search::glob))
        .route("/api/fuzzy", get(search::fuzzy))
        .route("/api/health", get(health::health))
        .route("/api/stats", get(stats::stats))
        .route("/api/reindex", post(reindex::reindex))
        .route("/api/base-path", get(health::base_path))
        .route("/api/file-server", get(health::file_server))
        .route("/api/feedback", get(health::feedback))
        .route(
            "/api/trending",
            get(trending::trending).delete(trending::delete_trending),
        )
        .merge(
            SwaggerUi::new("/swagger-ui")
                .url(
                    "/openapi.json",
                    ApiDoc::openapi_with_server(openapi_server.as_deref()),
                )
                .config(SwaggerConfig::new([openapi_json_path])),
        );

    // MCP / Streamable HTTP endpoint — shares the same engine/state as REST.
    let mcp_state = state.clone();
    let mcp_service: StreamableHttpService<PlocateMcpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(PlocateMcpHandler::new(mcp_state.clone())),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default().with_stateful_mode(false),
        );
    let inner = inner.nest_service("/mcp", mcp_service);

    if prefix.is_empty() {
        inner
            .fallback(frontend::frontend_fallback)
            .with_state(state)
    } else {
        // axum's `nest` does not match the bare prefix (e.g. `/search`), so
        // add an explicit permanent redirect to `/search/`. `/search/` and
        // any `/search/<deep/spa/route>` fall through to the outer fallback,
        // which strips the prefix and serves the embedded asset / SPA shell.
        let redirect_to = format!("{prefix}/");
        Router::new()
            .route(
                &prefix,
                get(move || async move { axum::response::Redirect::permanent(&redirect_to) }),
            )
            .nest(&prefix, inner)
            .fallback(frontend::frontend_fallback)
            .with_state(state)
    }
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::config::Config;
    use crate::state::AppState;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    fn test_config(public_base_url: Option<&str>) -> Config {
        Config {
            base_path: std::env::temp_dir(),
            bind: String::from("127.0.0.1:0"),
            db_path: Some(std::env::temp_dir().join("plocate-server-routes-test.db")),
            plocate_bin: String::from("plocate"),
            updatedb_bin: String::from("updatedb"),
            max_results: 100,
            max_concurrent_searches: 8,
            search_timeout_secs: 10,
            queue_timeout_secs: 5,
            fuzzy_candidate_cap: 1000,
            invalidate_stat_cache_on_reindex: true,
            updatedb_timeout_secs: 3600,
            file_server_url: None,
            feedback_email: None,
            instance_name: String::from("plocate"),
            trending_enabled: false,
            trending_window_secs: 86_400,
            trending_bucket_secs: 3_600,
            trending_min_query_len: 2,
            trending_top_n: 20,
            trending_blocklist_path: None,
            public_base_url: public_base_url.map(str::to_owned),
        }
    }

    async fn get(path: &str, public_base_url: Option<&str>) -> (StatusCode, Vec<u8>) {
        let state = AppState::new(&test_config(public_base_url)).unwrap();
        let response = router(state)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, body)
    }

    fn assert_openapi_json(status: StatusCode, body: &[u8]) {
        assert_eq!(status, StatusCode::OK);
        let json: Value = serde_json::from_slice(body).unwrap();
        let version = json.get("openapi").and_then(Value::as_str).unwrap();
        assert!(version.starts_with("3."));
        assert!(json.pointer("/paths/~1api~1trending/delete").is_some());
    }

    #[tokio::test]
    async fn root_openapi_json_serves_spec() {
        let (status, body) = get("/openapi.json", None).await;
        assert_openapi_json(status, &body);
    }

    #[tokio::test]
    async fn prefixed_openapi_json_serves_spec() {
        let (status, body) = get("/search/openapi.json", Some("/search")).await;
        assert_openapi_json(status, &body);
    }

    #[tokio::test]
    async fn prefixed_swagger_ui_fetches_prefixed_spec() {
        let (status, body) =
            get("/search/swagger-ui/swagger-initializer.js", Some("/search")).await;
        assert_eq!(status, StatusCode::OK);
        let js = String::from_utf8(body).unwrap();
        assert!(js.contains("/search/openapi.json"));
    }

    #[tokio::test]
    async fn delete_trending_persists_and_blocks_query() {
        let blocklist_path = std::env::temp_dir().join(format!(
            "plocate-server-routes-blocklist-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&blocklist_path);
        let mut config = test_config(None);
        config.trending_enabled = true;
        config.trending_blocklist_path = Some(blocklist_path.clone());
        let state = AppState::new(&config).unwrap();
        let tracker = state.trending.clone().unwrap();
        tracker.record("Cargo.toml");

        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/trending?q=Cargo.toml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(tracker.top_k(10).is_empty());
        tracker.record("cargo.toml");
        assert!(tracker.top_k(10).is_empty());
        let saved: Vec<String> =
            serde_json::from_slice(&std::fs::read(&blocklist_path).unwrap()).unwrap();
        assert_eq!(saved, vec!["cargo.toml"]);
        std::fs::remove_file(blocklist_path).unwrap();
    }
}
