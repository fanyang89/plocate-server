# Configuration

Source of truth: `src/config.rs`. Every CLI flag has a matching
`PLOCATE_SERVER_*` environment variable (provided by `clap`'s `env` feature).
Flags win over env; both win over defaults.

## Required

| Flag | Env | Notes |
| --- | --- | --- |
| `--base-path` | `PLOCATE_SERVER_BASE_PATH` | Root to index. Only paths under here are reachable. |

## Network & binaries

| Flag | Env | Default |
| --- | --- | --- |
| `--bind` | `PLOCATE_SERVER_BIND` | `127.0.0.1:8787` |
| `--db-path` | `PLOCATE_SERVER_DB_PATH` | see db-path resolution below |
| `--plocate-bin` | `PLOCATE_SERVER_PLOCATE_BIN` | `plocate` |
| `--updatedb-bin` | `PLOCATE_SERVER_UPDATEDB_BIN` | `updatedb` |

### `--db-path` resolution (`config.rs:134-165`)

When `--db-path` is not set, the server resolves the default as the first
writable location in this chain:

1. `$XDG_DATA_HOME/plocate-server/files.db`
2. `$HOME/.local/share/plocate-server/files.db`
3. `/var/lib/plocate-server/files.db` (systemd unit lands here)

### Mount-point traversal

Each reindex runs `updatedb --config-file /dev/null`, so the dedicated index
does not inherit `/etc/updatedb.conf`. All accessible content below
`--base-path` is indexed across bind mounts and other filesystem types. This
also includes directory names such as `.git` that a host-wide updatedb
configuration might otherwise exclude.

## Result limits

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--max-results` | `PLOCATE_SERVER_MAX_RESULTS` | `100` | Hard ceiling per request. `limit` query param is clamped to `1..=max_results`. |

## Concurrency & timeouts

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--max-concurrent-searches` | `PLOCATE_SERVER_MAX_CONCURRENT_SEARCHES` | `8` | `Semaphore` permits. Extra requests wait up to `queue-timeout-secs` then `503`. |
| `--search-timeout-secs` | `PLOCATE_SERVER_SEARCH_TIMEOUT_SECS` | `10` | Wall-clock per search. Exceeding returns `504`. |
| `--queue-timeout-secs` | `PLOCATE_SERVER_QUEUE_TIMEOUT_SECS` | `5` | Time spent waiting for a permit (distinct from search-timeout). |
| `--updatedb-timeout-secs` | `PLOCATE_SERVER_UPDATEDB_TIMEOUT_SECS` | `3600` | `updatedb` is killed past this. |

## Fuzzy & stat cache (HDD-relevant)

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--fuzzy-candidate-cap` | `PLOCATE_SERVER_FUZZY_CANDIDATE_CAP` | `1000` | Max paths pulled by `run_plocate_multi` before nucleo scoring. Lower on HDD to bound the synchronous stat fan-out. |
| `--invalidate-stat-cache-on-reindex` | `PLOCATE_SERVER_INVALIDATE_STAT_CACHE_ON_REINDEX` | `true` | Drop the moka stat cache after each reindex so types stay consistent. Set `false` on HDD to avoid a post-reindex cold window. |

See [hdd-tuning](./hdd-tuning.md) for the rationale — these knobs exist
because of the synchronous `stat` fan-out in `state.rs::parse_paths`.

## Trending searches

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--trending-enabled` | `PLOCATE_SERVER_TRENDING_ENABLED` | `true` | Show and record the REST search leaderboard. MCP searches are never counted. |
| `--trending-window-secs` | `PLOCATE_SERVER_TRENDING_WINDOW_SECS` | `86400` | Rolling count window. Counts reset on restart. |
| `--trending-bucket-secs` | `PLOCATE_SERVER_TRENDING_BUCKET_SECS` | `3600` | Sliding-window bucket size. |
| `--trending-min-query-len` | `PLOCATE_SERVER_TRENDING_MIN_QUERY_LEN` | `2` | Minimum normalized query length in characters. |
| `--trending-top-n` | `PLOCATE_SERVER_TRENDING_TOP_N` | `20` | Default leaderboard size, clamped to 1..=100. |
| `--trending-blocklist-path` | `PLOCATE_SERVER_TRENDING_BLOCKLIST_PATH` | next to `files.db` | Sorted JSON array of normalized queries permanently excluded from trending. |

The packaged service resolves the blocklist to
`/var/lib/plocate-server/trending-blocklist.json`. A malformed blocklist stops
startup instead of silently exposing blocked queries.

## UI / instance

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--instance-name` | `PLOCATE_SERVER_INSTANCE_NAME` | `plocate` | Label shown in the SPA header. |
| `--public-base-url` | `PLOCATE_SERVER_PUBLIC_BASE_URL` | *(unset)* | Mount everything under a path prefix or a canonical origin. Populates OpenAPI `servers`. |
| `--file-server-url` | `PLOCATE_SERVER_FILE_SERVER_URL` | *(unset)* | External file-server base; `/api/file-server` exposes it and MCP emits browseable URLs. |
| `--feedback-email` | `PLOCATE_SERVER_FEEDBACK_EMAIL` | *(unset)* | Contact email surfaced by `/api/feedback` and the SPA. |

### `--public-base-url`

Accepts either a path (`/search`) or a full URL
(`https://files.example.com/search`). A full URL is preferred — it also seeds
the OpenAPI `servers` field so Swagger UI's "Try it out" works out of the
box. See [deployment → path prefix](./deployment.md#mount-under-a-path-prefix).

## Packaging defaults are a subset

The env file shipped inside the RPM/pacman package
(`packaging/etc/plocate-server.env`) contains only the commonly-tuned subset:
`BASE_PATH`, `BIND`, `DB_PATH`, `TRENDING_BLOCKLIST_PATH`, vendored binary paths, `MAX_RESULTS`,
`MAX_CONCURRENT_SEARCHES`, `SEARCH_TIMEOUT_SECS`, `UPDATEDB_TIMEOUT_SECS`,
and a commented `RUST_LOG`. It intentionally omits the HDD knobs and the UI
optional flags — set those via the systemd unit override or a drop-in env
file.
