# MCP integration

plocate-server also speaks the
[Model Context Protocol](https://modelcontextprotocol.io/) over Streamable
HTTP at `POST /mcp`, so AI agents can search the indexed tree directly. Source:
`src/mcp.rs`.

`/mcp` is **stateless** — each JSON-RPC request is self-contained, no session
handshake required. It shares the same engine, concurrency cap, timeouts, and
input limits as the REST API. MCP searches are not counted by the trending
leaderboard.

## Tools

Three tools are exposed (`src/mcp.rs`):

| Tool | Purpose | Key args |
| --- | --- | --- |
| `search_files` (`:85`) | Substring (or glob) path search | `query`, `limit?`, `offset?`, `case_insensitive?`, `scope?` (`path` \| `basename`) |
| `glob` (`:121`) | Explicit glob pattern | `pattern`, `limit?`, `offset?`, `case_insensitive?` |
| `fuzzy_search` (`:153`) | Multi-keyword fuzzy search, fzf-style ranked by nucleo | `query`, `limit?`, `offset?`, `case_insensitive?` |

Each tool returns one match per line:

- a **fully-qualified browseable URL** when the server has
  `--file-server-url` configured (segments percent-encoded like
  `encodeURIComponent`; directories get a trailing `/`), or
- the **relative path** otherwise (directories get a trailing `/`).

Example output:

```
2 match(es)
src/main.rs
web/dist/
```

Or with `--file-server-url https://files.example.com`:

```
2 match(es)
https://files.example.com/src/main.rs
https://files.example.com/web/dist/
```

The first line reports the match count and a `(truncated, more exist)` marker
when the result set was capped.

## Raw usage

```bash
curl -s http://127.0.0.1:8787/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"search_files","arguments":{"query":"invoice"}}}'
```

List tools:

```bash
curl -s http://127.0.0.1:8787/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Agent configuration

The `/mcp` endpoint sits behind the same reverse proxy / auth boundary as the
REST API — point the agent at `https://your-host/mcp`. Snippets below come
from the SPA's installer (`web/src/lib/install.ts`).

### opencode

```bash
# Global scope
opencode mcp add plocate-server --url 'http://127.0.0.1:8787/mcp'
```

Project scope requires a manual edit of `.opencode/opencode.jsonc` — note the
key is `"mcp"`, not the Cursor-style `"mcpServers"`:

```jsonc
// .opencode/opencode.jsonc
{
  "mcp": {
    "plocate-server": { "type": "http", "url": "http://127.0.0.1:8787/mcp" }
  }
}
```

### Claude Code

```bash
# Global (user) scope
claude mcp add -s user --transport http plocate-server 'http://127.0.0.1:8787/mcp'

# Project (local) scope — default, no flag needed
claude mcp add --transport http plocate-server 'http://127.0.0.1:8787/mcp'
```

### Codex

```bash
# Global scope
codex mcp add plocate-server --url 'http://127.0.0.1:8787/mcp'
```

Project scope requires editing `.codex/config.toml`:

```toml
[mcp_servers.plocate-server]
url = "http://127.0.0.1:8787/mcp"
```

### Generic (Cursor / Continue / anything that reads `mcpServers`)

```json
{
  "mcpServers": {
    "plocate-server": {
      "type": "http",
      "url": "http://127.0.0.1:8787/mcp"
    }
  }
}
```

## Why agents should prefer `/mcp` over `grep`

The MCP server instructions (`src/mcp.rs:194-208`) tell agents exactly this:
the index is refreshed periodically, search is sub-millisecond even at 10M+
paths, and a scanning `grep`/`glob` tool would walk the filesystem. For path
lookups over a configured root, `search_files` / `glob` / `fuzzy_search` are
always faster.
