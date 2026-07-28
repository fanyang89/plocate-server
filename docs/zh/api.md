# REST API

所有路由均为 JSON，默认挂载在 `/api` 下（设置 `--public-base-url` 前缀后
为 `/{prefix}/api`）。源码：`src/routes/mod.rs:55-72`、`src/routes/*.rs`。

| 方法 | 路径 | 标签 | 用途 |
| --- | --- | --- | --- |
| GET | `/api/search` | search | 子串（或 glob）文件名/路径搜索 |
| GET | `/api/glob` | search | 显式 glob 搜索 |
| GET | `/api/fuzzy` | search | 多关键词模糊搜索，按相关度排序 |
| GET | `/api/health` | lifecycle | 索引与二进制健康状态 |
| GET | `/api/stats` | lifecycle | 进程 RSS/线程数、db 大小/mtime、上次 reindex |
| POST | `/api/reindex` | lifecycle | 触发后台 `updatedb` 运行 |
| GET | `/api/base-path` | lifecycle | 当前索引的根目录 |
| GET | `/api/file-server` | lifecycle | 外部文件服务基址（如已配置） |
| GET | `/api/feedback` | lifecycle | 联系邮箱（如已配置） |
| GET | `/api/trending` | lifecycle | 滚动窗口内的 REST 搜索热榜 |
| DELETE | `/api/trending` | lifecycle | 从热榜永久屏蔽关键词 |

另有自动生成的接口：

| URL | 入口 |
| --- | --- |
| `/openapi.json` | OpenAPI 3.0 规范 |
| `/swagger-ui/**` | Swagger UI |

## 通用输入限制（`src/limits.rs`）

- `q` / `pattern` ≤ **256 字符**
- `offset` ≤ **10000**
- `limit` 被钳制到 `1..=max_results`

## `/api/search`

| 参数 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `q` | 是 | — | 子串，或含 `*`/`?`/`[` 时作为 glob。多个空白分隔的模式按 AND 组合。 |
| `limit` | 否 | `max_results` | 钳制到 `1..=max_results`。 |
| `offset` | 否 | `0` | 分页偏移（0 起，最大 10000）。 |
| `case` | 否 | `true` | `true` 时大小写不敏感。 |
| `scope` | 否 | `path` | `path`（全路径）或 `basename` / `b` / `name`。 |

```bash
curl 'http://127.0.0.1:8787/api/search?q=invoice&limit=20'
curl 'http://127.0.0.1:8787/api/search?q=readme&scope=basename'
curl 'http://127.0.0.1:8787/api/search?q=README&case=false'
```

## `/api/glob`

| 参数 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `pattern` | 是 | — | glob 模式，如 `*2024*.log` 或 `**/Cargo.toml`。 |
| `limit` | 否 | `max_results` | |
| `offset` | 否 | `0` | |
| `case` | 否 | `true` | |

```bash
curl 'http://127.0.0.1:8787/api/glob?pattern=*2024*.log'
```

## `/api/fuzzy`

空白分隔的多个关键词按 AND 组合（每个关键词都必须出现在路径中），
然后通过 [nucleo-matcher](https://crates.io/crates/nucleo-matcher) 按
fzf 风格的模糊相关度排序。适合子串搜索返回空的多关键词查询。

| 参数 | 必填 | 默认 | 说明 |
| --- | --- | --- | --- |
| `q` | 是 | — | 空白分隔的关键词。 |
| `limit` | 否 | `max_results` | |
| `offset` | 否 | `0` | |
| `case` | 否 | `true` | |

```bash
curl 'http://127.0.0.1:8787/api/fuzzy?q=zookeeper%20rpm%20oe1'
```

## `/api/trending`

`GET /api/trending` 返回 `{ window_secs, items: [{ query, count }] }`，按计数
降序排列。只有成功的 REST 搜索会被计数；MCP 搜索不会进入热榜。

`DELETE /api/trending?q=<query>` 从所有当前计数桶中移除关键词，并将其归一化
形式原子写入磁盘屏蔽清单。之后的搜索仍正常执行，但该关键词无法再次进入热榜。
接口幂等，成功返回 `204`。

```bash
curl http://127.0.0.1:8787/api/trending | jq
curl -X DELETE 'http://127.0.0.1:8787/api/trending?q=cargo.toml'
```

删除接口没有应用层鉴权，会为所有用户修改全局热榜。若不希望公开用户使用此
能力，应在网络或反向代理层限制访问。恢复方式见
[部署运维](./deployment.md#热门搜索屏蔽清单)。

## `/api/health`

```bash
curl http://127.0.0.1:8787/api/health | jq
```

返回 `{ ok, db_present, plocate_available, updatedb_available }`。仅当 db
文件存在且两个二进制都能在 `PATH` 上解析到时，`ok` 才为 true。

## `/api/stats`

```bash
curl http://127.0.0.1:8787/api/stats | jq
```

返回进程 RSS / 线程数（`/proc/self/status`）、db 大小 + mtime、当前
`reindexing` 标志、以及上次 reindex 记录。

## `POST /api/reindex`

```bash
curl -X POST http://127.0.0.1:8787/api/reindex
```

- `200` —— 后台 reindex 已启动。
- `202` —— 已有 reindex 在运行，复用现有任务。

幂等：可安全地从 cron / systemd timer 调用。

## 响应结构

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
      "score": 123                   // 仅 fuzzy 返回；其他情况省略
    }
  ]
}
```

### 结果说明

- plocate 的索引只存 **路径** —— 无大小 / mtime / git 元数据。
- `type`/`kind`（`file` 还是 `directory`）在查询时由 `stat` 决定，并
  memoize 到进程内的 [moka](https://crates.io/crates/moka) 缓存
  （10 万条目 ≈ 10 MB），每次 reindex 后失效。
- `total_matched` 是 plocate 在请求上限（`offset + limit`）内返回的数量，
  **不是** 整个索引的精确总数。`truncated` 表示可能还有更多匹配。

## 查询语义

`/api/search` 把模式通过 `--` 分隔符传给 `plocate`（不走 shell，无注入
风险）。plocate 把模式视为：

- 不含 glob 元字符时为 **子串**，
- 含 `*`、`?` 或 `[` 时为 **glob**（需包成 `*...*` 才能同时匹配子串）。

多个模式按 AND 组合。完整语义见 `plocate(1)`。
