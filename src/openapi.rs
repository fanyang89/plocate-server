use utoipa::OpenApi;
use utoipa::openapi::Server;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "plocate-server",
        version = "0.5.0",
        description = "RESTful filename-search API server backed by a plocate trigram index.\n\nThe index lives on disk (built by updatedb), so a restart never rescans. Designed for very large trees (millions of files).",
        license(name = "MIT"),
    ),
    paths(
        crate::routes::search::search,
        crate::routes::search::glob,
        crate::routes::search::fuzzy,
        crate::routes::health::health,
        crate::routes::health::base_path,
        crate::routes::health::file_server,
        crate::routes::health::feedback,
        crate::routes::stats::stats,
        crate::routes::reindex::reindex,
        crate::routes::trending::trending,
        crate::routes::trending::delete_trending,
    ),
    components(schemas(
        crate::dto::FileItemDto,
        crate::dto::SearchResponse,
        crate::dto::HealthResponse,
        crate::dto::StatsResponse,
        crate::dto::StatsProcess,
        crate::dto::StatsIndex,
        crate::dto::ReindexRecordDto,
        crate::dto::BasePathResponse,
        crate::dto::ReindexResponse,
        crate::dto::FileServerResponse,
        crate::dto::FeedbackResponse,
        crate::dto::TrendingItem,
        crate::dto::TrendingResponse,
    )),
    tags(
        (name = "search", description = "Filename / path search via plocate"),
        (name = "lifecycle", description = "Index health, stats, and reindex"),
    ),
)]
pub struct ApiDoc;

impl ApiDoc {
    /// Build the OpenAPI document, optionally populated with a `servers`
    /// entry. When the server is mounted behind a path prefix (or canonical
    /// public URL), Swagger UI's "Try it out" needs `servers` set so it
    /// targets the prefixed endpoints instead of the root.
    pub fn openapi_with_server(server: Option<&str>) -> utoipa::openapi::OpenApi {
        let mut doc = Self::openapi();
        if let Some(url) = server.filter(|s| !s.is_empty()) {
            doc.servers = Some(vec![Server::new(url)]);
        }
        doc
    }
}
