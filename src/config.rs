use std::path::PathBuf;

use clap::Parser;

/// RESTful filename-search API server backed by a plocate database.
///
/// Builds and queries a dedicated plocate index for a single directory tree,
/// refreshed periodically by `updatedb`. The index lives on disk, so a process
/// restart never rescans.
#[derive(Debug, Clone, Parser)]
#[command(name = "plocate-server", version, about)]
pub struct Config {
    /// Root directory to index.
    #[arg(long, env = "PLOCATE_SERVER_BASE_PATH")]
    pub base_path: PathBuf,

    /// Bind address for the HTTP server.
    #[arg(long, env = "PLOCATE_SERVER_BIND", default_value = "127.0.0.1:8787")]
    pub bind: String,

    /// Path to the plocate database (created/refreshed by updatedb).
    #[arg(long, env = "PLOCATE_SERVER_DB_PATH")]
    pub db_path: Option<PathBuf>,

    /// Override the `plocate` binary path.
    #[arg(long, env = "PLOCATE_SERVER_PLOCATE_BIN", default_value = "plocate")]
    pub plocate_bin: String,

    /// Override the `updatedb` binary path.
    #[arg(long, env = "PLOCATE_SERVER_UPDATEDB_BIN", default_value = "updatedb")]
    pub updatedb_bin: String,

    /// Maximum results returned by a single search/glob call.
    #[arg(long, env = "PLOCATE_SERVER_MAX_RESULTS", default_value_t = 100)]
    pub max_results: usize,

    /// Maximum concurrent plocate query processes. Excess requests wait
    /// (backpressure) rather than spawning unbounded children.
    #[arg(
        long,
        env = "PLOCATE_SERVER_MAX_CONCURRENT_SEARCHES",
        default_value_t = 8
    )]
    pub max_concurrent_searches: usize,

    /// Per-phase timeout (seconds). Applied to (1) the plocate child process
    /// and (2) the per-path `stat` fan-out that follows. Worst-case request
    /// latency is therefore up to 2× this value. A plocate run exceeding it
    /// is killed and reported as a 504; the stat fan-out stops early and the
    /// response is returned with `truncated=true`. Note this does NOT bound
    /// the time spent waiting for a concurrency slot — see
    /// `--queue-timeout-secs` for that.
    #[arg(long, env = "PLOCATE_SERVER_SEARCH_TIMEOUT_SECS", default_value_t = 10)]
    pub search_timeout_secs: u64,

    /// Maximum time (seconds) a request waits for a concurrency slot before
    /// returning 503. Distinct from `--search-timeout-secs`: this bounds the
    /// queue wait (admission control), not the query itself. Set to 0 to wait
    /// forever (legacy behavior; not recommended — clients will silent-timeout
    /// first). 5 s is a reasonable default for HDD deployments where a single
    /// fuzzy query can occupy a slot for seconds.
    #[arg(long, env = "PLOCATE_SERVER_QUEUE_TIMEOUT_SECS", default_value_t = 5)]
    pub queue_timeout_secs: u64,

    /// Upper bound on the candidate set fed to the nucleo fuzzy ranker.
    /// plocate recalls candidates with multi-pattern AND semantics; this cap
    /// bounds the ranking pass AND the per-path stat fan-out that follows.
    /// On SSD the default 1000 is fine (stat is sub-millisecond). On HDD
    /// where each stat costs 5-20 ms, lowering to 200 cuts fuzzy latency
    /// 5× at the cost of recall on rare multi-token queries.
    #[arg(
        long,
        env = "PLOCATE_SERVER_FUZZY_CANDIDATE_CAP",
        default_value_t = 1000
    )]
    pub fuzzy_candidate_cap: usize,

    /// Whether to clear stat_cache when a reindex completes. Default true
    /// keeps results strictly consistent with the new index. On HDD, clearing
    /// means every post-reindex query pays the full stat waterfall (100-1000
    /// cold stats × HDD latency). Set to false to keep the cache warm across
    /// reindexes — at the cost of briefly reporting deleted directories with
    /// a trailing slash until natural LRU eviction catches up.
    #[arg(
        long,
        env = "PLOCATE_SERVER_INVALIDATE_STAT_CACHE_ON_REINDEX",
        default_value_t = true
    )]
    pub invalidate_stat_cache_on_reindex: bool,

    /// Per-reindex timeout (seconds). An updatedb run exceeding this is killed.
    /// Generous by default to accommodate very large trees (10M+ files).
    #[arg(
        long,
        env = "PLOCATE_SERVER_UPDATEDB_TIMEOUT_SECS",
        default_value_t = 3600
    )]
    pub updatedb_timeout_secs: u64,

    /// Base URL of an external file-browsing service (dufs / caddy file_server /
    /// nginx autoindex ...) serving the same tree as `--base-path`. When set,
    /// clients can build a browse link per search result by appending its
    /// relative path. Optional; omitted = no browse links.
    #[arg(long, env = "PLOCATE_SERVER_FILE_SERVER_URL")]
    pub file_server_url: Option<String>,

    /// Contact email surfaced in the web UI for bug reports and feedback.
    /// When unset, the feedback entry is hidden entirely.
    #[arg(long, env = "PLOCATE_SERVER_FEEDBACK_EMAIL")]
    pub feedback_email: Option<String>,

    /// Skill/MCP instance name surfaced in the web UI install dialog and used
    /// as the default name in generated `opencode mcp add` / `codex mcp add`
    /// commands. Must match `^[a-z0-9]+(-[a-z0-9]+)*$` (1-64 chars, lowercase
    /// alphanum + single hyphens). Defaults to "plocate".
    #[arg(long, env = "PLOCATE_SERVER_INSTANCE_NAME", default_value = "plocate")]
    pub instance_name: String,

    /// Enable the trending-searches leaderboard (`/api/trending` and the
    /// "hot searches" panel in the UI). Each successful search query bumps a
    /// rolling-window counter; the top-N most frequent queries are exposed.
    /// Counts are in-memory and reset on restart; blocked queries are persisted
    /// separately via `--trending-blocklist-path`.
    #[arg(long, env = "PLOCATE_SERVER_TRENDING_ENABLED", default_value_t = true)]
    pub trending_enabled: bool,

    /// Sliding-window length (seconds) for trending counts. Default 24h.
    /// Counts older than this drop out of the leaderboard.
    #[arg(
        long,
        env = "PLOCATE_SERVER_TRENDING_WINDOW_SECS",
        default_value_t = 86_400
    )]
    pub trending_window_secs: u64,

    /// Bucket size (seconds) for the trending sliding window. Default 1h.
    /// The window holds `window / bucket` buckets; advancing rotates them.
    /// Smaller buckets = fresher leaderboard, more rotation churn.
    #[arg(
        long,
        env = "PLOCATE_SERVER_TRENDING_BUCKET_SECS",
        default_value_t = 3_600
    )]
    pub trending_bucket_secs: u64,

    /// Minimum normalized query length (in chars) to be eligible for the
    /// trending board. Filters out single-character / accidental keystrokes.
    /// Default 2.
    #[arg(
        long,
        env = "PLOCATE_SERVER_TRENDING_MIN_QUERY_LEN",
        default_value_t = 2
    )]
    pub trending_min_query_len: usize,

    /// Default leaderboard size for `/api/trending` when no `?limit` is given.
    /// Clamped to 1..=100. Default 20.
    #[arg(long, env = "PLOCATE_SERVER_TRENDING_TOP_N", default_value_t = 20)]
    pub trending_top_n: usize,

    /// JSON file containing normalized keyword AND rules permanently excluded
    /// from the trending board. Defaults to `trending-blocklist.json` next to
    /// the plocate database.
    #[arg(long, env = "PLOCATE_SERVER_TRENDING_BLOCKLIST_PATH")]
    pub trending_blocklist_path: Option<PathBuf>,

    /// Public base URL the service is served under, when mounted behind a
    /// reverse proxy that does NOT strip the path prefix. Accepts either a
    /// path (`/search`) or a full URL (`https://host/search`). When set:
    ///  - All routes (REST, Swagger UI, OpenAPI, MCP, frontend) are served
    ///    under the path prefix.
    ///  - The OpenAPI `servers` field is populated (from the full URL when
    ///    given, else the path prefix) so Swagger UI "Try it out" works.
    ///  - The embedded SPA is rewritten at serve time to load assets and
    ///    call the API under the prefix.
    ///
    /// When unset (default), the server is mounted at `/` as before.
    #[arg(long, env = "PLOCATE_SERVER_PUBLIC_BASE_URL")]
    pub public_base_url: Option<String>,
}

impl Config {
    /// Resolved database path, defaulting next to a cache dir.
    pub fn resolved_db_path(&self) -> PathBuf {
        self.db_path.clone().unwrap_or_else(|| {
            let base = std::env::var("XDG_DATA_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
                    match home {
                        // HOME empty/unset (e.g. under systemd with no
                        // Home= set) — fall back to the canonical service
                        // data dir instead of writing into the CWD. Without
                        // this the db could land in /.local/share/... which
                        // is read-only under ProtectSystem=strict and would
                        // crash-loop the service.
                        Some(h) => Some(PathBuf::from(h).join(".local").join("share")),
                        None => {
                            tracing::warn!(
                                "neither XDG_DATA_HOME nor HOME is set; \
                                 defaulting db path to /var/lib/plocate-server"
                            );
                            Some(PathBuf::from("/var/lib/plocate-server"))
                        }
                    }
                })
                .expect("fallback always yields Some");
            base.join("plocate-server").join("files.db")
        })
    }

    pub fn resolved_trending_blocklist_path(&self) -> PathBuf {
        self.trending_blocklist_path.clone().unwrap_or_else(|| {
            self.resolved_db_path()
                .with_file_name("trending-blocklist.json")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::path::PathBuf;

    fn cfg(db_path: Option<PathBuf>) -> Config {
        Config {
            base_path: PathBuf::from("/tmp"),
            bind: String::from("127.0.0.1:8787"),
            db_path,
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
            trending_enabled: true,
            trending_window_secs: 86_400,
            trending_bucket_secs: 3_600,
            trending_min_query_len: 2,
            trending_top_n: 20,
            trending_blocklist_path: None,
            public_base_url: None,
        }
    }

    /// With an explicit --db-path, resolution is identity and does NOT touch
    /// the environment (so it is safe regardless of how the test binary is
    /// launched). The XDG/HOME fallback branches are covered by inspection:
    /// they only differ in which prefix the join uses, and the prefix logic
    /// is too environment-dependent to test deterministically in a shared
    /// process without `unsafe` env mutation (forbidden under edition 2024).
    #[test]
    fn resolved_db_path_explicit_wins() {
        assert_eq!(
            cfg(Some(PathBuf::from("/custom/files.db"))).resolved_db_path(),
            PathBuf::from("/custom/files.db")
        );
        // Unset case: function still returns a well-formed path; the exact
        // prefix depends on env vars at runtime, so we only assert suffix.
        let p = cfg(None).resolved_db_path();
        assert!(p.ends_with("plocate-server/files.db"));
    }

    #[test]
    fn blocklist_defaults_next_to_database() {
        let config = cfg(Some(PathBuf::from("/custom/files.db")));
        assert_eq!(
            config.resolved_trending_blocklist_path(),
            PathBuf::from("/custom/trending-blocklist.json")
        );
    }
}
