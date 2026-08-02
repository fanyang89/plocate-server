use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::process::Command;
use tokio::sync::Semaphore;

use moka::sync::Cache;

use crate::config::Config;
use crate::dto::{FileItemDto, SearchResponse};
use crate::error::{AppError, Result};
use crate::trending::TrendingTracker;

/// Indexing root + plocate database handles shared with every handler.
#[derive(Clone)]
pub struct AppState {
    pub base_path: Arc<PathBuf>,
    pub db_path: Arc<PathBuf>,
    pub plocate_bin: Arc<OsString>,
    pub updatedb_bin: Arc<OsString>,
    pub max_results: usize,
    pub file_server_url: Arc<Option<String>>,
    pub feedback_email: Arc<Option<String>>,
    /// Skill/MCP instance name surfaced via /api/health and the web UI.
    pub instance_name: Arc<String>,
    /// Per-path `is_dir` cache (stat results). Keyed by absolute path; values
    /// are valid for the current reindex window — cleared on reindex completion.
    /// `moka::sync::Cache` is `Clone` and shares its store internally, so no
    /// `Arc` wrapper is needed.
    stat_cache: Cache<String, bool>,
    reindexing: Arc<AtomicBool>,
    last_run: Arc<Mutex<Option<ReindexRecord>>>,
    search_concurrency: Arc<Semaphore>,
    /// Configured cap on concurrent plocate runs; stored alongside the
    /// semaphore so queue-timeout error messages can report in-use vs max
    /// without re-deriving it.
    max_concurrent_searches: usize,
    search_timeout: Duration,
    /// How long a request may wait for a concurrency slot before 503.
    /// Distinct from `search_timeout` (which bounds the plocate run itself).
    queue_timeout: Duration,
    /// Cap on fuzzy candidate recall (also bounds the stat fan-out).
    /// Configurable so HDD deployments can trade recall for latency.
    fuzzy_candidate_cap: usize,
    /// Whether to drop stat_cache on reindex completion. Default true;
    /// HDD deployments may set false to avoid the post-reindex cold window.
    invalidate_stat_cache_on_reindex: bool,
    updatedb_timeout: Duration,
    /// Public base URL prefix (e.g. "/search"); empty string means root mount.
    /// Used by the router to nest all routes and by the frontend to rewrite
    /// asset/API URLs at serve time.
    pub base_url_prefix: Arc<String>,
    /// Full public URL (scheme+host+prefix) when the operator provided one;
    /// used to populate OpenAPI `servers`. None when only a path prefix (or
    /// nothing) was given — clients then derive absolute URLs from their own
    /// origin at runtime.
    pub base_url_public: Arc<Option<String>>,
    /// Prefix-rewritten index.html bytes, computed once at startup when the
    /// server is mounted under a non-empty base URL prefix. None when the
    /// prefix is empty OR the frontend has not been built (served verbatim
    /// / "not built" path respectively).
    pub index_html_override: Arc<Option<Vec<u8>>>,
    /// Rolling-window trending-searches tracker. None when `--trending-enabled=false`.
    pub trending: Option<Arc<TrendingTracker>>,
}

/// Parsed `--public-base-url` value.
///
/// `prefix` is the path component normalized to "/" + segments with no
/// trailing slash, or "" for root mount. `public_url` is the original input
/// when it was a full URL (scheme://host[/path]); used for OpenAPI servers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseUrlConfig {
    pub prefix: String,
    pub public_url: Option<String>,
}

/// Parse a `--public-base-url` raw value into routing/runtime components.
///
/// Accepted forms:
/// - `None` / empty → root mount (prefix="", public_url=None)
/// - `/search` or `search` or `/search/` → prefix="/search", public_url=None
/// - `https://host/search` → prefix="/search", public_url=Some(input)
/// - `https://host` → prefix="", public_url=Some(input)
///
/// The prefix is used for in-process router nesting and SPA rewriting; the
/// full URL (when supplied) is preferred for the OpenAPI `servers` field so
/// Swagger UI "Try it out" targets the canonical public origin.
pub fn parse_base_url(raw: Option<&str>) -> BaseUrlConfig {
    let raw = match raw.map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => return BaseUrlConfig::default(),
    };

    // Full URL: split at the path so the router still nests under the path
    // component, but keep the original as the public canonical URL.
    if let Some(scheme_idx) = raw.find("://") {
        let after_scheme = &raw[scheme_idx + 3..];
        // Path starts at the first '/' after the host[:port].
        let path = match after_scheme.find('/') {
            Some(i) => &after_scheme[i..],
            None => "",
        };
        return BaseUrlConfig {
            prefix: normalize_prefix(path),
            public_url: Some(strip_trailing_slash(raw)),
        };
    }

    BaseUrlConfig {
        prefix: normalize_prefix(raw),
        public_url: None,
    }
}

/// Normalize a path component to "" or "/seg[/seg...]" with no trailing slash.
fn normalize_prefix(p: &str) -> String {
    let p = p.trim_start_matches('/');
    if p.is_empty() {
        return String::new();
    }
    let p = p.trim_end_matches('/');
    format!("/{p}")
}

fn strip_trailing_slash(s: &str) -> String {
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Upper bound on the stat cache size. Each entry is ~100 B, so this caps
/// memory near 10 MB. Normal use stays far below (only searched paths enter,
/// and the cache is cleared on every reindex unless
/// --invalidate-stat-cache-on-reindex=false).
const STAT_CACHE_CAPACITY: u64 = 100_000;

#[derive(Clone, Debug)]
pub struct ReindexRecord {
    pub started_at: SystemTime,
    pub duration_secs: f64,
    pub success: bool,
    pub error: Option<String>,
}

pub enum ReindexOutcome {
    Started,
    AlreadyRunning,
}

impl AppState {
    pub fn new(cfg: &Config) -> Result<Self> {
        let base_path = cfg
            .base_path
            .canonicalize()
            .map_err(|e| AppError::BadRequest(format!("base_path invalid: {e}")))?;
        let db_path = cfg.resolved_db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let trending_blocklist_path = cfg.resolved_trending_blocklist_path();
        if cfg.trending_enabled
            && let Some(parent) = trending_blocklist_path.parent()
        {
            std::fs::create_dir_all(parent)?;
        }
        crate::limits::validate_skill_name(&cfg.instance_name)?;
        let parsed = parse_base_url(cfg.public_base_url.as_deref());
        Ok(Self {
            base_path: Arc::new(base_path),
            db_path: Arc::new(db_path),
            plocate_bin: Arc::new(cfg.plocate_bin.clone().into()),
            updatedb_bin: Arc::new(cfg.updatedb_bin.clone().into()),
            max_results: cfg.max_results,
            file_server_url: Arc::new(normalize_file_server_url(cfg.file_server_url.as_deref())),
            feedback_email: Arc::new(normalize_feedback_email(cfg.feedback_email.as_deref())),
            instance_name: Arc::new(cfg.instance_name.clone()),
            stat_cache: Cache::new(STAT_CACHE_CAPACITY),
            reindexing: Arc::new(AtomicBool::new(false)),
            last_run: Arc::new(Mutex::new(None)),
            search_concurrency: Arc::new(Semaphore::new(cfg.max_concurrent_searches.max(1))),
            max_concurrent_searches: cfg.max_concurrent_searches.max(1),
            search_timeout: Duration::from_secs(cfg.search_timeout_secs),
            queue_timeout: Duration::from_secs(cfg.queue_timeout_secs),
            fuzzy_candidate_cap: cfg.fuzzy_candidate_cap.max(1),
            invalidate_stat_cache_on_reindex: cfg.invalidate_stat_cache_on_reindex,
            updatedb_timeout: Duration::from_secs(cfg.updatedb_timeout_secs),
            base_url_prefix: Arc::new(parsed.prefix.clone()),
            base_url_public: Arc::new(parsed.public_url.clone()),
            index_html_override: Arc::new(crate::routes::frontend::rewrite_index_html(
                &parsed.prefix,
            )),
            trending: if cfg.trending_enabled {
                Some(TrendingTracker::with_top_n(
                    cfg.trending_window_secs,
                    cfg.trending_bucket_secs,
                    cfg.trending_min_query_len,
                    cfg.trending_top_n,
                    trending_blocklist_path,
                )?)
            } else {
                None
            },
        })
    }

    pub fn db_exists(&self) -> bool {
        self.db_path.is_file()
    }

    pub fn last_run(&self) -> Option<ReindexRecord> {
        self.last_run.lock().ok().and_then(|g| g.clone())
    }

    pub fn is_reindexing(&self) -> bool {
        self.reindexing.load(Ordering::Acquire)
    }

    /// Configured cap on concurrent searches (>= 1). Exposed for diagnostics.
    pub fn max_concurrent_searches(&self) -> usize {
        self.max_concurrent_searches
    }

    /// Run a plocate query (substring by default; glob when the pattern
    /// contains glob metacharacters, per plocate's own rules).
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        case_insensitive: bool,
        basename_only: bool,
        track_trending: bool,
    ) -> Result<SearchResponse> {
        let started = Instant::now();
        if !self.db_exists() {
            return Ok(SearchResponse::empty());
        }
        let cap = offset.saturating_add(limit);
        // Bound concurrent plocate children; backpressure instead of fork-bomb.
        // The wait is bounded by `queue_timeout` — after that we 503 so
        // clients can retry rather than silent-timeout at their own end.
        let _permit = self.acquire_permit().await?;
        let raw = self
            .run_plocate(query, Some(cap), case_insensitive, basename_only)
            .await?;
        // Stat fan-out is moved to the blocking pool so it does not pin a
        // tokio worker thread. On HDD-class stat latency this is the
        // difference between 8 slots being stuck for seconds vs N slots
        // queueing freely behind a non-blocking async frontier.
        // The fan-out is bounded by `search_timeout` (same budget as the
        // plocate child): on expiry we stop stat'ing and return what we
        // have, marking the result `truncated`. Without this a single
        // deep-pagination request (offset+limit up to 10100 stats) could
        // pin a blocking worker for minutes on HDD.
        let base_path = self.base_path.clone();
        let stat_cache = self.stat_cache.clone();
        let stat_deadline = Instant::now() + self.search_timeout;
        let (items, stat_truncated) = tokio::task::spawn_blocking(move || {
            parse_paths(&raw, &base_path, &stat_cache, Some(stat_deadline))
        })
        .await
        .map_err(|e| AppError::Internal(format!("parse_paths join error: {e}")))?;
        let total_returned = items.len();
        let truncated = (total_returned == cap && cap > 0) || stat_truncated;
        let paged: Vec<FileItemDto> = items.into_iter().skip(offset).take(limit).collect();
        if track_trending && let Some(t) = &self.trending {
            t.record(query);
        }
        Ok(SearchResponse {
            total_matched: total_returned,
            truncated,
            items: paged,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Fuzzy search: recall candidates via plocate with multi-pattern AND
    /// semantics (one pattern per whitespace-separated token), then rank them
    /// with the nucleo fuzzy matcher (fzf-style scoring). Each result carries
    /// a `score`; results are ordered by descending score.
    ///
    /// This gives "search engine"-style multi-keyword matching: a query like
    /// `zookeeper rpm oe1` matches paths containing all three substrings, with
    /// better-matching paths (contiguous, prefix/word-boundary aligned) ranked
    /// first.
    pub async fn search_fuzzy(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        case_insensitive: bool,
        track_trending: bool,
    ) -> Result<SearchResponse> {
        let started = Instant::now();
        if !self.db_exists() {
            return Ok(SearchResponse::empty());
        }
        // Split into AND tokens; empty query → empty result.
        let tokens: Vec<&str> = query.split_whitespace().collect();
        if tokens.is_empty() {
            return Ok(SearchResponse::empty());
        }
        let _permit = self.acquire_permit().await?;
        let cap = self.fuzzy_candidate_cap;
        let raw = self
            .run_plocate_multi(&tokens, cap, case_insensitive)
            .await?;
        // Stat fan-out for fuzzy is up to `cap` paths (default 1000) —
        // the heaviest path through the server. Moving it off the tokio
        // worker is the single most important fix for HDD deployments.
        // Bounded by `search_timeout` so a cold-cache HDD scenario cannot
        // pin a blocking worker past the configured SLA.
        let base_path = self.base_path.clone();
        let stat_cache = self.stat_cache.clone();
        let stat_deadline = Instant::now() + self.search_timeout;
        let (items, stat_truncated) = tokio::task::spawn_blocking(move || {
            parse_paths(&raw, &base_path, &stat_cache, Some(stat_deadline))
        })
        .await
        .map_err(|e| AppError::Internal(format!("parse_paths join error: {e}")))?;
        let truncated = items.len() >= cap || stat_truncated;
        // Re-rank the recalled candidates with the basename-centric fused
        // scorer. The gate inside `rank::rerank` preserves the legacy "every
        // token must fuzzy-match the path" filter; structural signals (depth,
        // hidden, junk) and basename match strength then reorder the survivors.
        let ranked = crate::rank::rerank(query, case_insensitive, items);
        let total_matched = ranked.len();
        let paged: Vec<FileItemDto> = ranked.into_iter().skip(offset).take(limit).collect();
        if track_trending && let Some(t) = &self.trending {
            t.record(query);
        }
        Ok(SearchResponse {
            total_matched,
            truncated,
            items: paged,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Run plocate with multiple position arguments (AND semantics — a path
    /// must match every pattern). Output is NUL-separated, limited to `limit`.
    async fn run_plocate_multi(
        &self,
        patterns: &[&str],
        limit: usize,
        case_insensitive: bool,
    ) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&*self.plocate_bin);
        cmd.arg("-d").arg(&*self.db_path).arg("-N").arg("-0");
        if case_insensitive {
            cmd.arg("-i");
        }
        cmd.arg("-l").arg(limit.to_string());
        cmd.arg("--");
        for p in patterns {
            cmd.arg(enrich_glob(p));
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_with_timeout(&mut cmd, self.search_timeout, "plocate").await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.trim().is_empty() {
                return Ok(Vec::new());
            }
            return Err(AppError::Internal(format!(
                "plocate failed ({}): {stderr}",
                output.status
            )));
        }
        Ok(output.stdout)
    }

    async fn run_plocate(
        &self,
        pattern: &str,
        limit: Option<usize>,
        case_insensitive: bool,
        basename_only: bool,
    ) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&*self.plocate_bin);
        cmd.arg("-d").arg(&*self.db_path).arg("-N").arg("-0");
        if case_insensitive {
            cmd.arg("-i");
        }
        if basename_only {
            cmd.arg("-b");
        }
        if let Some(n) = limit {
            cmd.arg("-l").arg(n.to_string());
        }
        // plocate's glob anchors a pattern at the start of the full path, so a
        // bare `rust*json` could never match `/home/.../.rustc_info.json`.
        // When the pattern is a glob (contains * ? [) but is not already
        // wildcarded (`*`) or root-anchored (`/`), prepend `*` so it matches
        // anywhere in the path — matching the intuition of "find by name".
        let pattern = enrich_glob(pattern);
        cmd.arg("--").arg(pattern);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_with_timeout(&mut cmd, self.search_timeout, "plocate").await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // plocate exits 1 both for "no matches" and for genuine errors
            // (see plocate(1) EXIT STATUS). Disambiguate by stderr: an empty
            // stderr means no matches, so return an empty result rather than
            // surfacing a spurious error. Any real error (missing db, bad
            // flag, permission, ...) writes to stderr and is propagated.
            if stderr.trim().is_empty() {
                return Ok(Vec::new());
            }
            return Err(AppError::Internal(format!(
                "plocate failed ({}): {stderr}",
                output.status
            )));
        }
        Ok(output.stdout)
    }

    /// Wait for a concurrency slot, bounded by `queue_timeout`. Returns 503
    /// (QueueTimeout) on expiry so clients can retry / fail fast instead of
    /// silently timing out on their end while waiting for a slot.
    async fn acquire_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>> {
        match tokio::time::timeout(self.queue_timeout, self.search_concurrency.acquire()).await {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(_)) => Err(AppError::Internal(
                "search concurrency semaphore closed".into(),
            )),
            Err(_) => {
                // available_permits() is the FREE count, not in-use. Report
                // both available and the configured max so the operator can
                // tell saturation (0 free) from a transient slot churn.
                let max = self.max_concurrent_searches();
                let free = self.search_concurrency.available_permits().min(max);
                Err(AppError::QueueTimeout(format!(
                    "no concurrency slot within {}s ({} of {} slots in use)",
                    self.queue_timeout.as_secs(),
                    max - free,
                    max,
                )))
            }
        }
    }

    /// Trigger a background `updatedb` run if one is not already in flight.
    pub fn trigger_reindex(self) -> ReindexOutcome {
        if self.reindexing.swap(true, Ordering::AcqRel) {
            return ReindexOutcome::AlreadyRunning;
        }
        // Hold the flag via a guard so it is cleared on task drop, including
        // panic unwinding. Without this, any panic between swap(true) and the
        // final store(false) would leave the flag set forever and block all
        // future reindexes (every /api/reindex would return already-running).
        let guard = ReindexGuard {
            flag: Arc::clone(&self.reindexing),
        };
        let state = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let started = Instant::now();
            let started_at = SystemTime::now();
            let outcome = run_updatedb(&state).await;
            let rec = ReindexRecord {
                started_at,
                duration_secs: started.elapsed().as_secs_f64(),
                success: outcome.is_ok(),
                error: outcome.err().map(|e| e.to_string()),
            };
            tracing::info!(
                success = rec.success,
                duration_secs = %format!("{:.1}", rec.duration_secs),
                "reindex completed"
            );
            if let Ok(mut g) = state.last_run.lock() {
                *g = Some(rec);
            }
            // The index reflects the filesystem as of now, so stat results
            // from the previous window are stale. By default we drop them;
            // HDD deployments may set --invalidate-stat-cache-on-reindex=false
            // to keep the cache warm across reindexes (at the cost of briefly
            // reporting deleted directories with a trailing slash until LRU
            // eviction catches up).
            if state.invalidate_stat_cache_on_reindex {
                state.stat_cache.invalidate_all();
            }
            // flag is reset by `_guard` drop here.
        });
        ReindexOutcome::Started
    }
}

/// RAII guard that clears the `reindexing` flag on drop — including panic
/// unwinding — so a failed background reindex never wedges future runs.
struct ReindexGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for ReindexGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// plocate treats a pattern as a glob when it contains any of `*`, `?`, `[`.
/// Its glob is matched against the full path and anchored at the start, so
/// `rust*json` cannot match `/home/.../.rustc_info.json`. To make name-like
/// globs behave as "match anywhere in the path", prepend a leading `*` unless
/// the pattern already starts with `*` (already wildcarded) or `/`
/// (explicit root anchor, e.g. `/etc/*.conf`).
fn enrich_glob(pattern: &str) -> String {
    const GLOB_META: &[char] = &['*', '?', '['];
    if pattern.chars().next().is_some_and(|c| c == '*' || c == '/') {
        return pattern.to_string();
    }
    if !pattern.contains(GLOB_META) {
        return pattern.to_string();
    }
    format!("*{pattern}")
}

async fn run_updatedb(state: &AppState) -> Result<()> {
    let mut cmd = Command::new(&*state.updatedb_bin);
    cmd.args(updatedb_args(&state.base_path, &state.db_path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = run_with_timeout(&mut cmd, state.updatedb_timeout, "updatedb").await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!(
            "updatedb failed ({}): {stderr}",
            output.status
        )));
    }
    Ok(())
}

/// Build a dedicated updatedb invocation that cannot inherit host-wide pruning.
fn updatedb_args(base_path: &Path, db_path: &Path) -> Vec<OsString> {
    vec![
        "--config-file".into(),
        "/dev/null".into(),
        "-U".into(),
        base_path.as_os_str().to_owned(),
        "-o".into(),
        db_path.as_os_str().to_owned(),
        "--require-visibility".into(),
        "no".into(),
    ]
}

/// Spawn a command, wait for it with a timeout, and kill it if it exceeds the
/// deadline. Returns the captured output on success, or `Timeout` on expiry.
async fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output> {
    use tokio::io::AsyncReadExt;
    let mut child = cmd.spawn().map_err(|e| {
        AppError::Internal(format!("failed to run {label}: {e} (is it installed?)"))
    })?;
    // Take the pipes up front so we can drain them concurrently with wait(),
    // avoiding a deadlock when output exceeds the pipe buffer.
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let wait = child.wait();
    let drain_out = async {
        if let Some(s) = out.as_mut() {
            let _ = s.read_to_end(&mut stdout).await;
        }
    };
    let drain_err = async {
        if let Some(s) = err.as_mut() {
            let _ = s.read_to_end(&mut stderr).await;
        }
    };

    match tokio::time::timeout(timeout, async { tokio::join!(wait, drain_out, drain_err) }).await {
        Ok((status, _, _)) => Ok(std::process::Output {
            status: status.map_err(|e| AppError::Internal(format!("{label}: {e}")))?,
            stdout,
            stderr,
        }),
        Err(_) => {
            // Timed out — kill and reap the child to avoid leaking processes.
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(AppError::Timeout(format!(
                "{label} exceeded {}s",
                timeout.as_secs()
            )))
        }
    }
}

/// Parse NUL-separated plocate output into DTO items.
///
/// `stat_deadline` bounds the per-path `stat` fan-out: when `Instant::now()`
/// passes the deadline, parsing stops early and the second tuple element is
/// `true` so the caller can mark the response `truncated`. This prevents a
/// single request with thousands of cold stats (HDD) from pinning a blocking
/// pool worker indefinitely.
fn parse_paths(
    raw: &[u8],
    base_path: &Path,
    stat_cache: &Cache<String, bool>,
    stat_deadline: Option<Instant>,
) -> (Vec<FileItemDto>, bool) {
    if raw.is_empty() {
        return (Vec::new(), false);
    }
    let mut items = Vec::new();
    let mut stat_truncated = false;
    for chunk in raw.split(|&b| b == 0) {
        if chunk.is_empty() {
            continue;
        }
        if let Some(deadline) = stat_deadline
            && Instant::now() >= deadline
        {
            stat_truncated = true;
            break;
        }
        // plocate output is UTF-8 bytes; filesystems may contain non-UTF-8,
        // lossy-convert those rare cases.
        let abs = String::from_utf8_lossy(chunk).into_owned();
        items.push(build_item(&abs, base_path, stat_cache));
    }
    (items, stat_truncated)
}

fn build_item(abs: &str, base_path: &Path, stat_cache: &Cache<String, bool>) -> FileItemDto {
    // plocate does not tag directories in its output (no trailing slash, no
    // type field), so directory-ness is determined by stat at query time and
    // memoized in `stat_cache` for the duration of the current reindex window.
    let is_dir = is_dir_cached(stat_cache, abs);
    let abs_trimmed = abs
        .trim_end_matches('/')
        .trim_end_matches(std::path::MAIN_SEPARATOR);
    let relative = abs_trimmed
        .strip_prefix(base_path.to_string_lossy().as_ref())
        .map(|r| r.trim_start_matches('/').to_owned())
        .unwrap_or_else(|| abs_trimmed.to_string());
    let name = abs_trimmed
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(abs_trimmed)
        .to_string();
    FileItemDto {
        kind: if is_dir { "directory" } else { "file" }.into(),
        name,
        relative_path: relative,
        absolute_path: abs_trimmed.into(),
        score: None,
    }
}

/// Look up whether `abs` is a directory, using the shared stat cache. On a
/// cache miss, `symlink_metadata` is called (without following symlinks, so
/// symlinks — even to directories — are reported as files) and the result is
/// stored. Stat failures (deleted/unreadable) default to `false`.
fn is_dir_cached(cache: &Cache<String, bool>, abs: &str) -> bool {
    if let Some(v) = cache.get(abs) {
        return v;
    }
    let is_dir = std::fs::symlink_metadata(abs)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    cache.insert(abs.to_owned(), is_dir);
    is_dir
}

/// Validate an externally-configured file-server URL. Accepts http/https only;
/// trims trailing slashes so callers can safely append `/<relative_path>`.
/// Returns None (with a warning) on invalid input rather than failing startup.
fn normalize_file_server_url(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if !(raw.starts_with("http://") || raw.starts_with("https://")) {
        tracing::warn!(
            value = raw,
            "file_server_url must be http(s)://...; ignoring"
        );
        return None;
    }
    Some(raw.trim_end_matches('/').to_owned())
}

/// Trim the feedback email; empty input collapses to None so the UI hides the
/// feedback entry entirely.
fn normalize_feedback_email(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(raw.to_owned())
}

/// Read RSS (bytes) and thread count from `/proc/self/status` (Linux).
pub fn proc_status() -> io::Result<(u64, u32)> {
    let content = std::fs::read_to_string("/proc/self/status")?;
    let mut rss: Option<u64> = None;
    let mut threads: Option<u32> = None;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok());
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u32>().ok());
        }
        if rss.is_some() && threads.is_some() {
            break;
        }
    }
    Ok((rss.unwrap_or(0).saturating_mul(1024), threads.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::{
        BaseUrlConfig, Cache, ReindexGuard, enrich_glob, parse_base_url, parse_paths, updatedb_args,
    };
    use nucleo_matcher::Matcher;
    use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config, Utf32Str};
    use std::cmp::Reverse;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn enrich_glob_prepends_star_for_name_glob() {
        assert_eq!(enrich_glob("rust*json"), "*rust*json");
        assert_eq!(enrich_glob("src/*.rs"), "*src/*.rs");
        assert_eq!(enrich_glob("[Rr]eadme*"), "*[Rr]eadme*");
        assert_eq!(enrich_glob("?.rs"), "*?.rs");
    }

    #[test]
    fn enrich_glob_leaves_already_wildcarded_untouched() {
        assert_eq!(enrich_glob("*.rs"), "*.rs");
        assert_eq!(enrich_glob("**/2024/*.log"), "**/2024/*.log");
        assert_eq!(enrich_glob("*rust*json"), "*rust*json");
    }

    #[test]
    fn enrich_glob_leaves_root_anchored_untouched() {
        assert_eq!(enrich_glob("/etc/*.conf"), "/etc/*.conf");
        assert_eq!(enrich_glob("/rust*json"), "/rust*json");
    }

    #[test]
    fn enrich_glob_leaves_plain_substring_untouched() {
        // No glob metacharacters → plocate treats it as a substring; must not
        // add a leading `*` (that would turn it into a glob and change
        // semantics).
        assert_eq!(enrich_glob("Cargo.toml"), "Cargo.toml");
        assert_eq!(enrich_glob("config json"), "config json");
        assert_eq!(enrich_glob(""), "");
        assert_eq!(enrich_glob("*"), "*");
    }

    #[test]
    fn updatedb_args_disable_system_pruning() {
        assert_eq!(
            updatedb_args(Path::new("/base"), Path::new("/db/files.db")),
            vec![
                OsString::from("--config-file"),
                OsString::from("/dev/null"),
                OsString::from("-U"),
                OsString::from("/base"),
                OsString::from("-o"),
                OsString::from("/db/files.db"),
                OsString::from("--require-visibility"),
                OsString::from("no"),
            ]
        );
    }

    /// Smoke-test the fuzzy ranking used by `search_fuzzy`: a multi-token
    /// query must match paths containing all tokens, and paths with tighter
    /// (prefix/contiguous) alignment must outrank scattered matches.
    #[test]
    fn fuzzy_ranks_multi_token_query() {
        let paths = [
            "zookeeper/rpm/oe1/release.rpm", // all three, aligned
            "oe1/zookeeper/build.rpm",       // all three, reordered
            "zookeeper/rpm/other.tar",       // missing oe1
            "docs/zookeeper.md",             // only zookeeper
        ];
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(
            "zookeeper rpm oe1",
            CaseMatching::Ignore,
            Normalization::Smart,
        );
        let mut scored: Vec<(Option<u32>, &str)> = paths
            .iter()
            .map(|p| {
                (
                    pattern.score(Utf32Str::Ascii(p.as_bytes()), &mut matcher),
                    *p,
                )
            })
            .collect();
        scored.sort_by_key(|(s, _)| Reverse(*s));
        // The two paths containing all three tokens rank above the partials.
        assert_eq!(scored[0].1, "zookeeper/rpm/oe1/release.rpm");
        assert_eq!(scored[1].1, "oe1/zookeeper/build.rpm");
        // Partials do not match (score is None) and sink to the bottom.
        assert!(scored[2].0.is_none());
    }

    /// `parse_paths` with no deadline returns every chunk and never reports
    /// stat-truncation.
    #[test]
    fn parse_paths_no_deadline_returns_all() {
        let cache: Cache<String, bool> = Cache::new(10);
        let raw = b"a\0b\0\0c\0";
        let (items, truncated) = parse_paths(raw, Path::new("/"), &cache, None);
        assert_eq!(items.len(), 3);
        assert!(!truncated);
    }

    /// `parse_paths` with a deadline in the past must stop before stat'ing any
    /// path and report `truncated=true`. This is the HDD-safety contract: a
    /// single request cannot pin a blocking worker past the configured budget.
    #[test]
    fn parse_paths_expired_deadline_truncates() {
        let cache: Cache<String, bool> = Cache::new(10);
        let raw = b"/nonexistent/a\0/nonexistent/b\0/nonexistent/c\0";
        let past = Instant::now() - Duration::from_secs(1);
        let (items, truncated) = parse_paths(raw, Path::new("/"), &cache, Some(past));
        assert!(
            items.is_empty(),
            "no path should be stat'ed past the deadline"
        );
        assert!(truncated);
    }

    /// `ReindexGuard` clears the flag on drop. This is the panic-safety
    /// contract: even if the background task unwinds, the next `trigger_reindex`
    /// must be able to start a new run instead of seeing `already-running`
    /// forever.
    #[test]
    fn reindex_guard_clears_flag_on_drop() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let g = ReindexGuard {
                flag: Arc::clone(&flag),
            };
            // Simulate trigger_reindex's swap(true) followed by task body.
            assert!(!flag.swap(true, Ordering::AcqRel));
            drop(g);
        }
        assert!(
            !flag.load(Ordering::Acquire),
            "flag must be cleared on drop"
        );
        // And a fresh "trigger" must see the cleared flag.
        assert!(!flag.swap(true, Ordering::AcqRel));
    }

    #[test]
    fn parse_base_url_none_or_empty() {
        assert_eq!(parse_base_url(None), BaseUrlConfig::default());
        assert_eq!(parse_base_url(Some("")), BaseUrlConfig::default());
        assert_eq!(parse_base_url(Some("   ")), BaseUrlConfig::default());
    }

    #[test]
    fn parse_base_url_bare_path_normalizes() {
        assert_eq!(parse_base_url(Some("/search")).prefix.as_str(), "/search");
        // No leading slash, trailing slash, surrounding whitespace — all
        // collapse to the same normalized form.
        assert_eq!(parse_base_url(Some("search")).prefix, "/search");
        assert_eq!(parse_base_url(Some("/search/")).prefix, "/search");
        assert_eq!(parse_base_url(Some(" /search/ ")).prefix, "/search");
        // Multi-segment prefix is preserved.
        assert_eq!(
            parse_base_url(Some("/tools/search")).prefix,
            "/tools/search"
        );
        // Bare slash → root mount (empty prefix).
        assert_eq!(parse_base_url(Some("/")).prefix, "");
        assert!(parse_base_url(Some("/search")).public_url.is_none());
    }

    #[test]
    fn parse_base_url_full_url_keeps_canonical_and_extracts_path() {
        let r = parse_base_url(Some("https://example.com/search"));
        assert_eq!(r.prefix, "/search");
        assert_eq!(r.public_url.as_deref(), Some("https://example.com/search"));

        // http + port + trailing slash
        let r = parse_base_url(Some("http://host:8080/search/"));
        assert_eq!(r.prefix, "/search");
        assert_eq!(r.public_url.as_deref(), Some("http://host:8080/search"));

        // URL with no path component → root mount, but canonical URL kept
        // (useful when the operator wants OpenAPI servers to point at the
        // host without a path).
        let r = parse_base_url(Some("https://example.com"));
        assert_eq!(r.prefix, "");
        assert_eq!(r.public_url.as_deref(), Some("https://example.com"));
    }
}
