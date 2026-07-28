# REST API

All routes are JSON, mounted under `/api` by default (or `/{prefix}/api` when
`--public-base-url` sets a prefix). Source: `src/routes/mod.rs:55-72`,
`src/routes/*.rs`.

| Method | Path | Tag | Purpose |
| --- | --- | --- | --- |
| GET | `/api/search` | search | Substring (or glob) filename/path search |
| GET | `/api/glob` | search | Explicit glob search |
| GET | `/api/fuzzy` | search | Multi-keyword fuzzy search, relevance-ranked |
| GET | `/api/health` | lifecycle | Index + binary health |
| GET | `/api/stats` | lifecycle | Process RSS/threads, db size/mtime, last reindex |
| POST | `/api/reindex` | lifecycle | Trigger a background `updatedb` run |
| GET | `/api/base-path` | lifecycle | Currently indexed root |
| GET | `/api/file-server` | lifecycle | External file-server base URL (if configured) |
| GET | `/api/feedback` | lifecycle | Contact email (if configured) |
| GET | `/api/trending` | lifecycle | REST search leaderboard for the rolling window |
| DELETE | `/api/trending` | lifecycle | Permanently block a query from the leaderboard |

Plus the auto-generated surfaces:

| URL | Surface |
| --- | --- |
| `/openapi.json` | OpenAPI 3.0 spec |
| `/swagger-ui/**` | Swagger UI |

## Shared input limits (`src/limits.rs`)

- `q` / `pattern` ≤ **256 chars**
- `offset` ≤ **10000**
- `limit` is clamped to `1..=max_results`

## `/api/search`

| Param | Required | Default | Notes |
| --- | --- | --- | --- |
| `q` | yes | — | Substring, or glob if it contains `*`/`?`/`[`. Multiple whitespace-separated patterns are AND-ed. |
| `limit` | no | `max_results` | Clamped to `1..=max_results`. |
| `offset` | no | `0` | Pagination (0-based, max 10000). |
| `case` | no | `true` | Case-insensitive when `true`. |
| `scope` | no | `path` | `path` (full path) or `basename` / `b` / `name`. |

```bash
curl 'http://127.0.0.1:8787/api/search?q=invoice&limit=20'
curl 'http://127.0.0.1:8787/api/search?q=readme&scope=basename'
curl 'http://127.0.0.1:8787/api/search?q=README&case=false'
```

## `/api/glob`

| Param | Required | Default | Notes |
| --- | --- | --- | --- |
| `pattern` | yes | — | Glob pattern, e.g. `*2024*.log` or `**/Cargo.toml`. |
| `limit` | no | `max_results` | |
| `offset` | no | `0` | |
| `case` | no | `true` | |

```bash
curl 'http://127.0.0.1:8787/api/glob?pattern=*2024*.log'
```

## `/api/fuzzy`

Whitespace-separated tokens are AND-ed (every token must appear in the path),
then ranked by fzf-style fuzzy relevance via
[nucleo-matcher](https://crates.io/crates/nucleo-matcher). Best for queries
where plain substring search returns nothing.

| Param | Required | Default | Notes |
| --- | --- | --- | --- |
| `q` | yes | — | Whitespace-separated keywords. |
| `limit` | no | `max_results` | |
| `offset` | no | `0` | |
| `case` | no | `true` | |

```bash
curl 'http://127.0.0.1:8787/api/fuzzy?q=zookeeper%20rpm%20oe1'
```

## `/api/trending`

`GET /api/trending` returns `{ window_secs, items: [{ query, count }] }`, sorted
by descending count. Only successful REST searches are counted; MCP searches
never enter the leaderboard.

`DELETE /api/trending?q=<query>` removes the query from every active count
bucket and atomically adds its normalized form to the on-disk blocklist. Later
searches still run normally but cannot make that query trend again. The call is
idempotent and returns `204`.

```bash
curl http://127.0.0.1:8787/api/trending | jq
curl -X DELETE 'http://127.0.0.1:8787/api/trending?q=cargo.toml'
```

The delete endpoint has no application-level authentication and changes the
global leaderboard for every user. Restrict access at the network or reverse
proxy layer when public users should not have this capability. See
[deployment](./deployment.md#trending-query-blocklist) for recovery.

## `/api/health`

```bash
curl http://127.0.0.1:8787/api/health | jq
```

Returns `{ ok, db_present, plocate_available, updatedb_available }`. `ok` is
true only when the db file exists and both binaries resolve on `PATH`.

## `/api/stats`

```bash
curl http://127.0.0.1:8787/api/stats | jq
```

Returns process RSS / thread count (`/proc/self/status`), db size + mtime,
the current `reindexing` flag, and the last reindex record.

## `POST /api/reindex`

```bash
curl -X POST http://127.0.0.1:8787/api/reindex
```

- `200` — reindex started in the background.
- `202` — a reindex was already running; the existing one is reused.

Idempotent: safe to call from a cron / systemd timer.

## Response shape

```jsonc
{
  "total_matched": 42,
  "truncated": false,
  "items": [
    {
      "kind": "file",                // "file" | "directory"
      "name": "main.rs",
      "relative_path": "src/main.rs",
      "absolute_path": "/srv/files/src/main.rs",
      "score": 123                   // fuzzy only; omitted elsewhere
    }
  ]
}
```

### Notes on results

- plocate's index stores **paths only** — no size / mtime / git metadata.
- `type`/`kind` (`file` vs `directory`) is determined by `stat` at query time
  and memoized in an in-process [moka](https://crates.io/crates/moka) cache
  (100k entries ≈ 10 MB), invalidated on every reindex.
- `total_matched` is the count plocate returned up to the requested cap
  (`offset + limit`), **not** an exact total over the whole index. `truncated`
  indicates more matches likely exist.

## Query semantics

`/api/search` passes the pattern to `plocate` after a `--` separator (no
shell, so no injection). plocate treats a pattern as:

- a **substring** if it has no glob metacharacters,
- a **glob** if it contains `*`, `?`, or `[` (wrap in `*...*` to also match
  substrings).

Multiple patterns are AND-ed. See `plocate(1)` for the full semantics.
