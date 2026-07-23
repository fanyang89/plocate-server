# 配置参考

事实来源：`src/config.rs:12-132`。每个 CLI flag 都有对应的 `PLOCATE_SERVER_*`
环境变量（由 `clap` 的 `env` 特性提供）。优先级：flag > 环境变量 > 默认值。

## 必填项

| Flag | 环境变量 | 说明 |
| --- | --- | --- |
| `--base-path` | `PLOCATE_SERVER_BASE_PATH` | 要索引的根目录。只有此根之下的路径可达。 |

## 网络与二进制

| Flag | 环境变量 | 默认值 |
| --- | --- | --- |
| `--bind` | `PLOCATE_SERVER_BIND` | `127.0.0.1:8787` |
| `--db-path` | `PLOCATE_SERVER_DB_PATH` | 见下方 db-path 解析 |
| `--plocate-bin` | `PLOCATE_SERVER_PLOCATE_BIN` | `plocate` |
| `--updatedb-bin` | `PLOCATE_SERVER_UPDATEDB_BIN` | `updatedb` |

### `--db-path` 解析规则（`config.rs:134-165`）

未显式设置 `--db-path` 时，服务器按以下顺序选择第一个可写位置：

1. `$XDG_DATA_HOME/plocate-server/files.db`
2. `$HOME/.local/share/plocate-server/files.db`
3. `/var/lib/plocate-server/files.db`（systemd unit 落点）

### 挂载点遍历

每次 reindex 都以 `updatedb --config-file /dev/null` 运行，因此专用索引不会
继承 `/etc/updatedb.conf`。`--base-path` 下所有可访问内容都会跨越 bind mount
和其他文件系统类型被索引；这也包括全局 updatedb 配置通常会排除的 `.git` 等目录名。

## 结果限制

| Flag | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--max-results` | `PLOCATE_SERVER_MAX_RESULTS` | `100` | 单次请求硬上限。`limit` 查询参数被钳制到 `1..=max_results`。 |

## 并发与超时

| Flag | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--max-concurrent-searches` | `PLOCATE_SERVER_MAX_CONCURRENT_SEARCHES` | `8` | 信号量许可数。超额请求最多等待 `queue-timeout-secs` 后返回 `503`。 |
| `--search-timeout-secs` | `PLOCATE_SERVER_SEARCH_TIMEOUT_SECS` | `10` | 单次搜索的墙钟时间。超时返回 `504`。 |
| `--queue-timeout-secs` | `PLOCATE_SERVER_QUEUE_TIMEOUT_SECS` | `5` | 等待许可的时间（与 search-timeout 不同）。 |
| `--updatedb-timeout-secs` | `PLOCATE_SERVER_UPDATEDB_TIMEOUT_SECS` | `3600` | 超过此时间 `updatedb` 会被杀死。 |

## 模糊匹配与 stat 缓存（HDD 相关）

| Flag | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--fuzzy-candidate-cap` | `PLOCATE_SERVER_FUZZY_CANDIDATE_CAP` | `1000` | nucleo 打分前 `run_plocate_multi` 拉取的最大路径数。HDD 上调低以约束同步 stat 扇出。 |
| `--invalidate-stat-cache-on-reindex` | `PLOCATE_SERVER_INVALIDATE_STAT_CACHE_ON_REINDEX` | `true` | 每次 reindex 后丢弃 moka stat 缓存以保持类型一致。HDD 上设 `false` 可避免 reindex 后的冷窗口。 |

详见 [HDD 调优](./hdd-tuning.md) —— 这些旋钮的存在正是由于
`state.rs::parse_paths` 中的同步 `stat` 扇出。

## UI / 实例

| Flag | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--instance-name` | `PLOCATE_SERVER_INSTANCE_NAME` | `plocate` | SPA 顶部显示的实例名。 |
| `--public-base-url` | `PLOCATE_SERVER_PUBLIC_BASE_URL` | *(未设)* | 将所有路由挂到路径前缀或规范 origin 下。填充 OpenAPI `servers`。 |
| `--file-server-url` | `PLOCATE_SERVER_FILE_SERVER_URL` | *(未设)* | 外部文件服务基址；`/api/file-server` 暴露它，MCP 输出可浏览 URL。 |
| `--feedback-email` | `PLOCATE_SERVER_FEEDBACK_EMAIL` | *(未设)* | 联系邮箱，由 `/api/feedback` 与 SPA 暴露。 |

### `--public-base-url`

接受路径（`/search`）或完整 URL（`https://files.example.com/search`）。
推荐使用完整 URL —— 它还会填充 OpenAPI `servers` 字段，使 Swagger UI 的
"Try it out" 开箱即用。详见
[部署运维 → 路径前缀挂载](./deployment.md#挂载到路径前缀)。

## 打包默认值是子集

RPM/pacman 包内附带的环境文件
（`packaging/etc/plocate-server.env`）只包含常用调优子集：`BASE_PATH`、
`BIND`、`DB_PATH`、内置二进制路径、`MAX_RESULTS`、
`MAX_CONCURRENT_SEARCHES`、`SEARCH_TIMEOUT_SECS`、`UPDATEDB_TIMEOUT_SECS`，
以及注释掉的 `RUST_LOG`。它刻意省略了 HDD 调优旋钮和 UI 可选 flag ——
请通过 systemd unit override 或 env drop-in 文件设置它们。
