# Deployment

Three supported paths, from lightest to heaviest:

1. **Bare binary** — `scp` the static musl binary + a hand-written systemd unit.
2. **Linux package** — `task rpm` / `task pacman` / `task deb`, installs the binary, vendored
   musl `plocate`/`updatedb`, systemd units, and an env file.
3. **Container** — run the static binary under any OCI runtime; the binary has
   no libc dependency, so `FROM scratch` works.

## Bare binary

```bash
# 1. Build & install
task build
sudo install -m 0755 target/x86_64-unknown-linux-musl/release/plocate-server \
    /usr/local/bin/plocate-server

# 2. Ensure plocate is on the host
sudo dnf install plocate

# 3. Dedicated unprivileged user
sudo useradd -r -s /usr/sbin/nologin -d /var/lib/plocate-server -M plocate-server
sudo install -d -o plocate-server -g plocate-server /var/lib/plocate-server

# 4. Install the unit (shipped at deploy/plocate-server.service)
sudo install -m 0644 deploy/plocate-server.service /etc/systemd/system/
sudo systemctl daemon-reload

# 5. Point it at your tree (override ExecStart without editing the file)
sudo systemctl edit plocate-server
#   [Service]
#   ExecStart=
#   ExecStart=/usr/local/bin/plocate-server \
#       --base-path=/srv/files \
#       --db-path=/var/lib/plocate-server/files.db
#
#   Periodic refresh: set up a timer/cron that POSTs to /api/reindex,
#   or use the maintenance dialog in the web UI for manual refresh.

sudo systemctl enable --now plocate-server
```

## Linux packages (RPM / pacman / deb)

Built by [nfpm](https://github.com/goreleaser/nfpm) via `Taskfile.yml`:

```bash
task rpm           # → dist/plocate-server-<ver>.x86_64.rpm
task pacman        # → dist/plocate-server-<ver>-1-x86_64.pkg.tar.zst
task deb           # → dist/plocate-server-<ver>_amd64.deb
task packages      # all three
```

The package bundles three binaries, three systemd units, an env file, and
the state dir:

| Payload | Path |
| --- | --- |
| `plocate-server` | `/usr/bin/plocate-server` |
| vendored musl `plocate` + `updatedb` | `/usr/libexec/plocate-server/` |
| `plocate-server.service` | `/usr/lib/systemd/system/` |
| `plocate-server-updatedb.{service,timer}` | `/usr/lib/systemd/system/` |
| env defaults (`config(noreplace)`) | `/etc/plocate-server/plocate-server.env` |
| state dir | `/var/lib/plocate-server` |

The `updatedb` timer (`OnCalendar=daily`, `RandomizedDelaySec=1h`,
`Persistent=true`) POSTs to `/api/reindex` every day — no cron setup needed.

Configure via the env file and/or `systemctl edit plocate-server`.

## Resource limits (cgroup v2)

Both `deploy/plocate-server.service` and the packaged unit apply these. Edit
to taste — they are the reason a busy foreground service is never starved.

| Directive | Value | Effect |
| --- | --- | --- |
| `MemoryMax` | `1G` | Hard RSS ceiling. Server holds no index in RAM. |
| `MemoryHigh` | `800M` | Reclaim pressure starts here. |
| `AmbientCapabilities` | `CAP_DAC_READ_SEARCH` | Lets `updatedb` traverse the whole tree without root. |
| `CPUWeight` | `20` | Low weight vs default 100 — yields CPU under load. |
| `CPUQuota` | `200%` | Hard ceiling — reindex uses at most 2 cores even when idle. |
| `Nice` | `19` | Lowest static priority. |
| `IOSchedulingClass` | `idle` | Disk IO only served when no one else wants it. |
| `IOSchedulingPriority` | `7` | Lowest IO priority within `idle`. |

`Nice=19` + `IOSchedulingClass=idle` are the strongest guarantees that a busy
foreground service is never starved; `updatedb` runs inherit these too.
`CPUQuota=200%` caps reindex at 2 cores even on an idle host; `CPUWeight` /
`Nice` still yield under contention.

> **HDD caveat:** `IOSchedulingClass=idle` is a real guarantee on HDD but a
> no-op on multi-queue NVMe (the kernel only wires `io.cost.*` there). On SSD
> the bound comes from `CPUQuota` + `Nice`. See
> [hdd-tuning](./hdd-tuning.md).

## Permissions

`updatedb` must read every file under `--base-path` to index it. The unit
grants `CAP_DAC_READ_SEARCH` so the unprivileged `plocate-server` user can do
this without running as root. The resulting `files.db` is owned by
`plocate-server`, so the `plocate` child can read it back directly.

## Trending query blocklist

The web UI's delete action permanently blocks similar queries from the global
trending leaderboard. The packaged service stores normalized keyword AND rules in
`/var/lib/plocate-server/trending-blocklist.json` as a JSON array:

```json
[
  "cargo.toml",
  "release iso"
]
```

Each string is split on whitespace. A trending query matches when it contains
every keyword as a case-insensitive substring, in any order. Thus `release iso`
blocks `iso release x86` and `pre-release linux.iso`. Existing string entries
use the same AND semantics after upgrading.

The list is loaded at startup. To inspect it, use `jq` or another JSON reader.
To unblock a rule, stop the service, remove the entry from the JSON array, and
start the service again. Editing while the service is running is unsupported:
a concurrent UI block can overwrite out-of-process changes.

```bash
sudo systemctl stop plocate-server
sudo jq . /var/lib/plocate-server/trending-blocklist.json
# Edit the JSON array, then:
sudo systemctl start plocate-server
```

Search counts remain in memory and reset on restart. The blocklist persists.
`DELETE /api/trending` has no application-level authentication, so every user
who can reach it can globally block a query. Apply reverse
proxy access controls if that is not intended for a deployment.

## Mount under a path prefix

By default the server mounts at `/`. To serve it under a sub-path (e.g.
`https://files.example.com/search/`), pass `--public-base-url` and configure
the reverse proxy to forward **without** stripping the prefix:

```bash
plocate-server --base-path /srv/files --public-base-url /search
# Full URL is preferred — also seeds OpenAPI `servers`:
plocate-server --base-path /srv/files \
    --public-base-url https://files.example.com/search
```

Every surface then moves under the prefix: `/search/api/...`,
`/search/swagger-ui`, `/search/openapi.json`, `/search/mcp`, and the SPA at
`/search/`. The bare prefix (`/search`) redirects to `/search/`; unprefixed
paths (`/api/health`, `/`) return 404 so misrouted requests are obvious.

**nginx** — no trailing slash on `proxy_pass`, so the prefix is preserved:

```nginx
location /search/ {
    proxy_pass http://127.0.0.1:8787;
}
```

**Caddy** — `reverse_proxy` forwards the full URI by default:

```caddy
files.example.com {
    reverse_proxy /search/* 127.0.0.1:8787
}
```

## Observe

```bash
curl http://127.0.0.1:8787/api/stats  | jq    # RSS, db size/mtime, last reindex
curl http://127.0.0.1:8787/api/health | jq
systemctl status plocate-server
journalctl -u plocate-server -f
```

## Development setup

See [AGENTS.md](../../AGENTS.md) for the workspace layout and pre-commit
checks. In short:

```bash
git config core.hooksPath .githooks   # one-time, enables fmt check on commit
task check                            # cargo check
task web-dev                          # Vite dev server (proxies API to :8787)
cargo test --all                      # workspace tests
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs the same suite.
