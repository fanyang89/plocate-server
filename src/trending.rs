//! Sliding-window "trending queries" tracker.
//!
//! Counts how often each (normalized) search query is issued within a rolling
//! window, so the UI can surface a "hot searches" board — useful right after a
//! popular ISO / RPM drops and many users hunt for the same paths.
//!
//! Implementation: fixed-bucket rotation. The window is divided into N equal
//! buckets (e.g. 24 × 1h for a 24h window). Every search increments the count
//! for the *current* bucket under a normalized key. A background tokio task
//! advances the current-bucket pointer every `bucket_secs` and clears the
//! bucket it lands on (which by then is one full window old). Reading the
//! leaderboard sums all N buckets per key.
//!
//! Properties:
//!   - Write cost: one HashMap lookup + u64 bump (mutex held briefly).
//!   - Read cost: O(N buckets × distinct terms) aggregation.
//!   - Sliding precision: each count is at most one bucket older than the
//!     nominal window; a fresh "drop" becomes visible immediately in the
//!     current bucket, and stale counts vanish one bucket after the window.
//!   - Counts are in-memory and reset on restart. The operator-managed query
//!     blocklist is persisted separately and survives restarts.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Hard upper bound on `?limit` to bound read-side work.
pub const MAX_TOP_N: usize = 100;

pub struct TrendingTracker {
    buckets: Vec<Mutex<HashMap<String, u64>>>,
    blocklist: Mutex<Blocklist>,
    current: AtomicUsize,
    num_buckets: usize,
    bucket_secs: u64,
    window_secs: u64,
    min_len: usize,
    default_top_n: usize,
}

struct Blocklist {
    path: PathBuf,
    queries: HashSet<String>,
}

impl TrendingTracker {
    /// Construct a tracker and spawn its bucket-rotation task.
    ///
    /// `window_secs` / `bucket_secs` are clamped to >= 1; the bucket count is
    /// `(window / bucket).max(1)`. If `bucket_secs > window_secs`, the whole
    /// window collapses to a single bucket (still correct, just coarser).
    pub fn with_top_n(
        window_secs: u64,
        bucket_secs: u64,
        min_len: usize,
        default_top_n: usize,
        blocklist_path: PathBuf,
    ) -> io::Result<Arc<Self>> {
        let bucket_secs = bucket_secs.max(1);
        let window_secs = window_secs.max(1);
        let num_buckets = ((window_secs / bucket_secs).max(1)) as usize;
        let default_top_n = default_top_n.clamp(1, MAX_TOP_N);
        let blocked_queries = load_blocklist(&blocklist_path)?;
        let me = Arc::new(Self {
            buckets: (0..num_buckets)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
            blocklist: Mutex::new(Blocklist {
                path: blocklist_path,
                queries: blocked_queries,
            }),
            current: AtomicUsize::new(0),
            num_buckets,
            bucket_secs,
            window_secs,
            min_len,
            default_top_n,
        });
        me.spawn_rotator();
        Ok(me)
    }

    /// Build a tracker for tests without spawning the rotator.
    #[cfg(test)]
    pub fn for_test(min_len: usize) -> Arc<Self> {
        Self::for_test_with_path(
            min_len,
            std::env::temp_dir().join(format!(
                "plocate-server-trending-test-unused-{}.json",
                std::process::id()
            )),
        )
        .unwrap()
    }

    #[cfg(test)]
    fn for_test_with_path(min_len: usize, blocklist_path: PathBuf) -> io::Result<Arc<Self>> {
        let blocked_queries = load_blocklist(&blocklist_path)?;
        Ok(Arc::new(Self {
            buckets: (0..3).map(|_| Mutex::new(HashMap::new())).collect(),
            blocklist: Mutex::new(Blocklist {
                path: blocklist_path,
                queries: blocked_queries,
            }),
            current: AtomicUsize::new(0),
            num_buckets: 3,
            bucket_secs: 1,
            window_secs: 3,
            min_len,
            default_top_n: 20,
        }))
    }

    fn spawn_rotator(self: &Arc<Self>) {
        // Detached task: tokio keeps it alive until the runtime shuts down,
        // which is exactly the process lifetime we want. The task advances
        // the current-bucket pointer every `bucket_secs` and clears the
        // bucket it lands on (which by construction is one full window old).
        let me = Arc::clone(self);
        let period = std::time::Duration::from_secs(me.bucket_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            // The first tick fires immediately; skip it so we don't rotate
            // at startup before the first bucket has collected anything.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                me.rotate();
            }
        });
    }

    /// Advance to the next bucket and clear it. Order matters: clear first,
    /// then publish the new index. Recorders read `current` then lock the
    /// bucket at that index, so a stale reader can at worst land in the
    /// bucket that just became the oldest (its counts linger one extra cycle
    /// — harmless).
    fn rotate(&self) {
        let next = (self.current.load(Ordering::Acquire) + 1) % self.num_buckets;
        {
            let mut g = self.buckets[next].lock().unwrap();
            g.clear();
        }
        self.current.store(next, Ordering::Release);
    }

    /// Normalize and count a query. No-op for too-short queries.
    ///
    /// Normalization is `trim().to_lowercase()` — collapses cosmetic
    /// whitespace/case differences without erasing meaningful path structure.
    pub fn record(&self, raw: &str) {
        let Some(key) = normalize(raw) else {
            return;
        };
        if key.chars().count() < self.min_len {
            return;
        }
        let blocklist = self.blocklist.lock().unwrap();
        if blocklist.queries.contains(&key) {
            return;
        }
        let idx = self.current.load(Ordering::Acquire);
        let mut g = self.buckets[idx].lock().unwrap();
        *g.entry(key).or_insert(0) += 1;
    }

    /// Persistently exclude a query and remove all of its current counts.
    pub fn block(&self, raw: &str) -> io::Result<()> {
        let key = normalize(raw).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "query must not be empty")
        })?;
        let mut blocklist = self.blocklist.lock().unwrap();
        if !blocklist.queries.contains(&key) {
            let mut next = blocklist.queries.clone();
            next.insert(key.clone());
            persist_blocklist(&blocklist.path, &next)?;
            blocklist.queries = next;
        }
        for bucket in &self.buckets {
            bucket.lock().unwrap().remove(&key);
        }
        Ok(())
    }

    /// Aggregate counts across the whole window and return the top-`k` queries
    /// by descending count, ties broken alphabetically.
    pub fn top_k(&self, k: usize) -> Vec<(String, u64)> {
        let k = k.clamp(1, MAX_TOP_N);
        let mut agg: HashMap<String, u64> = HashMap::new();
        for b in &self.buckets {
            for (key, cnt) in b.lock().unwrap().iter() {
                *agg.entry(key.clone()).or_insert(0) += cnt;
            }
        }
        let mut v: Vec<(String, u64)> = agg.into_iter().collect();
        // Sort by count desc, then key asc for stable, deterministic ordering.
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(k);
        v
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    pub fn default_top_n(&self) -> usize {
        self.default_top_n
    }
}

fn normalize(raw: &str) -> Option<String> {
    let query = raw.trim();
    (!query.is_empty()).then(|| query.to_lowercase())
}

fn load_blocklist(path: &Path) -> io::Result<HashSet<String>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error),
    };
    let queries: Vec<String> = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid trending blocklist {}: {error}", path.display()),
        )
    })?;
    Ok(queries
        .into_iter()
        .filter_map(|query| normalize(&query))
        .collect())
}

fn persist_blocklist(path: &Path, queries: &HashSet<String>) -> io::Result<()> {
    let mut sorted: Vec<&str> = queries.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let bytes = serde_json::to_vec_pretty(&sorted)
        .map_err(|error| io::Error::other(format!("serialize blocklist: {error}")))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trending-blocklist.json");
    let temporary_path = path.with_file_name(format!(".{file_name}.tmp"));
    let mut file = File::create(&temporary_path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_PATH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn blocklist_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "plocate-server-{name}-{}-{}.json",
            std::process::id(),
            NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn record_increments_current_bucket() {
        let t = TrendingTracker::for_test(2);
        t.record("Cargo.toml");
        t.record("cargo.toml"); // case-folded onto the same key
        t.record("Cargo.toml");
        t.record("x"); // below min_len → dropped
        let top = t.top_k(10);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0], ("cargo.toml".to_string(), 3));
    }

    #[test]
    fn rotation_clears_oldest_bucket() {
        let t = TrendingTracker::for_test(2);
        t.record("alpha");
        t.record("alpha");
        // Rotate once: current advances, the bucket we just filled is now the
        // oldest but still in the window.
        t.rotate();
        t.record("beta");
        let top = t.top_k(10);
        let map: HashMap<String, u64> = top.into_iter().collect();
        assert_eq!(map.get("alpha").copied(), Some(2));
        assert_eq!(map.get("beta").copied(), Some(1));

        // Rotate twice more (total 3 = num_buckets): the alpha bucket is now
        // cleared and its counts drop out of the window entirely.
        t.rotate();
        t.rotate();
        let map: HashMap<String, u64> = t.top_k(10).into_iter().collect();
        assert_eq!(map.get("alpha"), None);
    }

    #[test]
    fn top_k_is_sorted_and_truncated() {
        let t = TrendingTracker::for_test(2);
        for _ in 0..5 {
            t.record("busy");
        }
        for _ in 0..3 {
            t.record("mid");
        }
        t.record("rare");
        let top = t.top_k(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "busy");
        assert_eq!(top[0].1, 5);
        assert_eq!(top[1].0, "mid");
        assert_eq!(top[1].1, 3);
    }

    #[test]
    fn trim_normalizes_whitespace() {
        let t = TrendingTracker::for_test(2);
        t.record("  Cargo.toml  ");
        t.record("Cargo.toml");
        let map: HashMap<String, u64> = t.top_k(10).into_iter().collect();
        assert_eq!(map.get("cargo.toml").copied(), Some(2));
    }

    #[test]
    fn min_len_uses_char_count_not_byte_count() {
        let t = TrendingTracker::for_test(2);
        // '长度' is 2 chars / 6 bytes — must count as length 2, not be
        // rejected by an accidental byte-length check.
        t.record("长度");
        let top = t.top_k(10);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0, "长度");
    }

    #[test]
    fn empty_window_returns_empty_leaderboard() {
        let t = TrendingTracker::for_test(2);
        assert!(t.top_k(10).is_empty());
    }

    #[test]
    fn block_removes_all_counts_and_prevents_rerecording() {
        let path = blocklist_path("blocked-counts");
        let t = TrendingTracker::for_test_with_path(2, path.clone()).unwrap();
        t.record("Cargo.toml");
        t.rotate();
        t.record("cargo.toml");

        t.block("  CARGO.TOML  ").unwrap();
        assert!(t.top_k(10).is_empty());
        t.record("cargo.toml");
        assert!(t.top_k(10).is_empty());

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn blocklist_is_sorted_and_loaded_on_restart() {
        let path = blocklist_path("persisted-blocks");
        let t = TrendingTracker::for_test_with_path(2, path.clone()).unwrap();
        t.block("Zulu").unwrap();
        t.block(" alpha ").unwrap();
        t.block("ALPHA").unwrap();

        let saved: Vec<String> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved, vec!["alpha", "zulu"]);

        let reloaded = TrendingTracker::for_test_with_path(2, path.clone()).unwrap();
        reloaded.record("Alpha");
        reloaded.record("zulu");
        assert!(reloaded.top_k(10).is_empty());

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_blocklist_is_rejected() {
        let path = blocklist_path("malformed-blocks");
        std::fs::write(&path, b"not json").unwrap();
        let error = match TrendingTracker::for_test_with_path(2, path.clone()) {
            Ok(_) => panic!("malformed blocklist should fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("invalid trending blocklist"));
        std::fs::remove_file(path).unwrap();
    }

    /// Construction with degenerate inputs must not panic and must collapse
    /// to a single bucket (still a valid, if coarse, window).
    #[tokio::test]
    async fn new_collapses_degenerate_ratios() {
        let t = TrendingTracker::with_top_n(
            0,
            0,
            2,
            20,
            std::env::temp_dir().join("plocate-server-trending-degenerate.json"),
        )
        .unwrap();
        assert!(t.window_secs() >= 1);
        t.record("hello");
        assert_eq!(t.top_k(10).len(), 1);
    }
}
