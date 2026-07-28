# 部署运维

支持三种部署路径，由轻到重：

1. **裸二进制** —— `scp` 静态 musl 二进制 + 手写 systemd unit。
2. **Linux 软件包** —— `task rpm` / `task pacman` / `task deb`，安装二进制、内置 musl
   `plocate`/`updatedb`、systemd unit 和 env 文件。
3. **容器** —— 在任何 OCI 运行时下跑静态二进制；二进制无 libc 依赖，
   `FROM scratch` 即可。

## 裸二进制

```bash
# 1. 构建并安装
task build
sudo install -m 0755 target/x86_64-unknown-linux-musl/release/plocate-server \
    /usr/local/bin/plocate-server

# 2. 确保宿主已装 plocate
sudo dnf install plocate

# 3. 专用非特权用户
sudo useradd -r -s /usr/sbin/nologin -d /var/lib/plocate-server -M plocate-server
sudo install -d -o plocate-server -g plocate-server /var/lib/plocate-server

# 4. 安装 unit（位于 deploy/plocate-server.service）
sudo install -m 0644 deploy/plocate-server.service /etc/systemd/system/
sudo systemctl daemon-reload

# 5. 指向你的目录树（用 override 改 ExecStart，不编辑原文件）
sudo systemctl edit plocate-server
#   [Service]
#   ExecStart=
#   ExecStart=/usr/local/bin/plocate-server \
#       --base-path=/srv/files \
#       --db-path=/var/lib/plocate-server/files.db
#
#   定期刷新：配置一个 timer/cron POST 到 /api/reindex，
#   或用 Web UI 的维护对话框手动刷新。

sudo systemctl enable --now plocate-server
```

## Linux 软件包（RPM / pacman / deb）

由 [nfpm](https://github.com/goreleaser/nfpm) 通过 `Taskfile.yml` 构建：

```bash
task rpm           # → dist/plocate-server-<ver>.x86_64.rpm
task pacman        # → dist/plocate-server-<ver>-1-x86_64.pkg.tar.zst
task deb           # → dist/plocate-server-<ver>_amd64.deb
task packages      # 三者都构建
```

软件包内含三个二进制、三个 systemd unit、一个 env 文件和状态目录：

| 载荷 | 路径 |
| --- | --- |
| `plocate-server` | `/usr/bin/plocate-server` |
| 内置 musl `plocate` + `updatedb` | `/usr/libexec/plocate-server/` |
| `plocate-server.service` | `/usr/lib/systemd/system/` |
| `plocate-server-updatedb.{service,timer}` | `/usr/lib/systemd/system/` |
| env 默认值（`config(noreplace)`） | `/etc/plocate-server/plocate-server.env` |
| 状态目录 | `/var/lib/plocate-server` |

`updatedb` timer（`OnCalendar=daily`、`RandomizedDelaySec=1h`、
`Persistent=true`）每天 POST 到 `/api/reindex` —— 无需另配 cron。

通过 env 文件和/或 `systemctl edit plocate-server` 进行配置。

## 资源限制（cgroup v2）

`deploy/plocate-server.service` 和打包 unit 都应用以下约束。可按需调整 ——
它们是"绝不饿死前台服务"的保障。

| 指令 | 值 | 效果 |
| --- | --- | --- |
| `MemoryMax` | `1G` | RSS 硬上限。服务器不在内存中持索引。 |
| `MemoryHigh` | `800M` | 此处开始回收压力。 |
| `AmbientCapabilities` | `CAP_DAC_READ_SEARCH` | 让 `updatedb` 不需 root 即可遍历整棵树。 |
| `CPUWeight` | `20` | 低于默认 100 的权重 —— 负载下让出 CPU。 |
| `CPUQuota` | `200%` | 硬上限 —— 即使空闲 reindex 也最多用 2 核。 |
| `Nice` | `19` | 最低静态优先级。 |
| `IOSchedulingClass` | `idle` | 仅当无其他进程需要磁盘时才服务 IO。 |
| `IOSchedulingPriority` | `7` | `idle` 类内的最低 IO 优先级。 |

`Nice=19` + `IOSchedulingClass=idle` 是"绝不饿死繁忙前台服务"的最强保障；
`updatedb` 运行也继承这些。`CPUQuota=200%` 即使在空闲主机上也把 reindex
限制在 2 核；`CPUWeight` / `Nice` 在争用时仍然让出。

> **HDD 注意：** `IOSchedulingClass=idle` 在 HDD 上是真实保障，但在多队列
> NVMe 上无效（内核只接 `io.cost.*`）。SSD 上的约束来自 `CPUQuota` +
> `Nice`。详见 [HDD 调优](./hdd-tuning.md)。

## 权限

`updatedb` 必须读取 `--base-path` 下的每个文件才能索引。unit 授予
`CAP_DAC_READ_SEARCH`，使非特权 `plocate-server` 用户无需以 root 运行即可
完成遍历。生成的 `files.db` 归 `plocate-server` 所有，因此 `plocate` 子
进程可直接读回。

## 热门搜索屏蔽清单

Web UI 的删除操作会从全局热榜永久屏蔽同类查询。软件包服务将规范化后的关键词
AND 规则保存到 `/var/lib/plocate-server/trending-blocklist.json`，格式为 JSON 数组：

```json
[
  "cargo.toml",
  "release iso"
]
```

每个字符串按空白拆分。热榜查询只要以任意顺序包含全部关键词子串即为匹配，
且不区分大小写。因此 `release iso` 会屏蔽 `iso release x86` 和
`pre-release linux.iso`。升级后，已有字符串条目也使用相同的 AND 语义。

服务只在启动时加载该文件。检查时可使用 `jq` 或其他 JSON 读取工具。要解除
规则，先停止服务，从 JSON 数组中移除对应项，再启动服务。不支持运行中直接
编辑：同时发生的 UI 屏蔽操作可能覆盖进程外修改。

```bash
sudo systemctl stop plocate-server
sudo jq . /var/lib/plocate-server/trending-blocklist.json
# 编辑 JSON 数组，然后：
sudo systemctl start plocate-server
```

搜索计数保存在内存中，重启后清零；屏蔽清单会保留。`DELETE /api/trending`
没有应用层鉴权，任何能访问它的用户都能为全局热榜屏蔽关键词。
不符合部署预期时，应在反向代理层增加访问控制。

## 挂载到路径前缀

默认挂载在 `/`。要挂到子路径下（如
`https://files.example.com/search/`），传 `--public-base-url` 并把反向代理
配置为 **不** 剥离前缀转发：

```bash
plocate-server --base-path /srv/files --public-base-url /search
# 完整 URL 更优 —— 同时填充 OpenAPI `servers`：
plocate-server --base-path /srv/files \
    --public-base-url https://files.example.com/search
```

所有接口都会移到前缀下：`/search/api/...`、`/search/swagger-ui`、
`/search/openapi.json`、`/search/mcp` 以及 `/search/` 处的 SPA。裸前缀
（`/search`）会重定向到 `/search/`；无前缀的路径（`/api/health`、`/`）
返回 404，让错路由的请求一目了然。

**nginx** —— `proxy_pass` 不加尾斜杠，从而保留前缀：

```nginx
location /search/ {
    proxy_pass http://127.0.0.1:8787;
}
```

**Caddy** —— `reverse_proxy` 默认转发完整 URI：

```caddy
files.example.com {
    reverse_proxy /search/* 127.0.0.1:8787
}
```

## 观测

```bash
curl http://127.0.0.1:8787/api/stats  | jq    # RSS、db 大小/mtime、上次 reindex
curl http://127.0.0.1:8787/api/health | jq
systemctl status plocate-server
journalctl -u plocate-server -f
```

## 开发环境

工作区布局和 pre-commit 检查见 [AGENTS.md](../../AGENTS.md)。简要：

```bash
git config core.hooksPath .githooks   # 一次性，启用提交时的 fmt 检查
task check                            # cargo check
task web-dev                          # Vite dev server（API 代理到 :8787）
cargo test --all                      # 工作区测试
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

CI（`.github/workflows/ci.yml`）运行同一套检查。
