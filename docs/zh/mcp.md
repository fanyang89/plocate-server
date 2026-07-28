# MCP 集成

plocate-server 还通过 Streamable HTTP 在 `POST /mcp` 上说
[模型上下文协议（MCP）](https://modelcontextprotocol.io/)，使 AI Agent 能
直接搜索索引树。源码：`src/mcp.rs`。

`/mcp` 是 **无状态** 的 —— 每个 JSON-RPC 请求自包含，无需会话握手。它与
REST API 共享同一套引擎、并发上限、超时和输入限制。MCP 搜索不会计入热门
搜索榜。

## 工具

暴露三个工具（`src/mcp.rs`）：

| 工具 | 用途 | 关键参数 |
| --- | --- | --- |
| `search_files`（`:85`） | 子串（或 glob）路径搜索 | `query`、`limit?`、`offset?`、`case_insensitive?`、`scope?`（`path` \| `basename`） |
| `glob`（`:121`） | 显式 glob 模式 | `pattern`、`limit?`、`offset?`、`case_insensitive?` |
| `fuzzy_search`（`:153`） | 多关键词模糊搜索，nucleo 按 fzf 风格排序 | `query`、`limit?`、`offset?`、`case_insensitive?` |

每个工具每行返回一条匹配：

- 当服务器配置了 `--file-server-url` 时返回 **完整可浏览 URL**（路径段按
  `encodeURIComponent` 编码；目录加尾 `/`），或
- 否则返回 **相对路径**（目录加尾 `/`）。

输出示例：

```
2 match(es)
src/main.rs
web/dist/
```

配置 `--file-server-url https://files.example.com` 时：

```
2 match(es)
https://files.example.com/src/main.rs
https://files.example.com/web/dist/
```

第一行报告匹配数，结果集被截断时附带 `(truncated, more exist)` 标记。

## 裸调用

```bash
curl -s http://127.0.0.1:8787/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"search_files","arguments":{"query":"invoice"}}}'
```

列出工具：

```bash
curl -s http://127.0.0.1:8787/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Agent 配置

`/mcp` 端点与 REST API 位于同一反向代理 / 鉴权边界之后 —— 把 Agent 指向
`https://your-host/mcp`。下列代码片段来自 SPA 的安装器
（`web/src/lib/install.ts`）。

### opencode

```bash
# 全局范围
opencode mcp add plocate-server --url 'http://127.0.0.1:8787/mcp'
```

项目范围需手动编辑 `.opencode/opencode.jsonc` —— 注意键名是 `"mcp"`，不是
Cursor 风格的 `"mcpServers"`：

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
# 全局（user）范围
claude mcp add -s user --transport http plocate-server 'http://127.0.0.1:8787/mcp'

# 项目（local）范围 —— 默认，无需 flag
claude mcp add --transport http plocate-server 'http://127.0.0.1:8787/mcp'
```

### Codex

```bash
# 全局范围
codex mcp add plocate-server --url 'http://127.0.0.1:8787/mcp'
```

项目范围需编辑 `.codex/config.toml`：

```toml
[mcp_servers.plocate-server]
url = "http://127.0.0.1:8787/mcp"
```

### 通用（Cursor / Continue / 任何读取 `mcpServers` 的工具）

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

## 为什么 Agent 应优先用 `/mcp` 而非 `grep`

MCP 服务器的指令（`src/mcp.rs:194-208`）正是这样告诉 Agent 的：索引定期
刷新、搜索在 10M+ 路径下仍为亚毫秒、而扫描式的 `grep`/`glob` 工具会遍历
文件系统。在配置好的根下做路径查找，`search_files` / `glob` /
`fuzzy_search` 永远更快。
