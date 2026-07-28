import i18n from "@/i18n"
import { withPrefix } from "@/lib/config"

export type FileItem = {
  type: string
  name: string
  relative_path: string
  absolute_path: string
  score?: number
}

export type SearchResponse = {
  total_matched: number
  truncated: boolean
  items: FileItem[]
  elapsed_ms: number
}

export type HealthResponse = {
  ok: boolean
  base_path: string
  instance_name: string
  db_present: boolean
  db_mtime_unix: number | null
  db_size_bytes: number | null
  reindexing: boolean
  plocate_available: boolean
  updatedb_available: boolean
}

export type ReindexRecord = {
  started_at_unix: number
  duration_secs: number
  success: boolean
  error: string | null
}

export type StatsResponse = {
  index: {
    db_present: boolean
    db_size_bytes: number | null
    db_mtime_unix: number | null
    reindexing: boolean
  }
  last_reindex: ReindexRecord | null
}

export type ReindexResponse = {
  status: "started" | "already-running"
}

export type FileServerConfig = {
  url: string | null
}

export type FeedbackConfig = {
  email: string | null
}

export type TrendingItem = {
  query: string
  count: number
}

export type TrendingResponse = {
  window_secs: number
  items: TrendingItem[]
}

export class SearchError extends Error {
  readonly retryable: boolean

  constructor(message: string, retryable: boolean) {
    super(message)
    this.name = "SearchError"
    this.retryable = retryable
  }
}

export async function fetchSearch(
  q: string,
  signal: AbortSignal,
): Promise<SearchResponse> {
  const params = new URLSearchParams({
    q,
    limit: "100",
    case: "true",
  })

  let res: Response
  try {
    res = await fetch(`${withPrefix("/api/fuzzy")}?${params.toString()}`, {
      signal,
      headers: { accept: "application/json" },
    })
  } catch (e) {
    if ((e as Error).name === "AbortError") throw e
    throw new SearchError(i18n.t("errors.searchConnect"), true)
  }

  if (!res.ok) {
    const retryable = res.status >= 500 || res.status === 429
    let detail = i18n.t("errors.requestFailed", { status: res.status })
    try {
      const body = await res.json()
      if (body?.error) detail = body.error
    } catch {
      // ignore JSON parse errors
    }
    throw new SearchError(detail, retryable)
  }

  try {
    return (await res.json()) as SearchResponse
  } catch {
    throw new SearchError(i18n.t("errors.parseFailed"), false)
  }
}

export async function fetchHealth(
  signal: AbortSignal,
): Promise<HealthResponse> {
  const res = await fetch(withPrefix("/api/health"), {
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`health failed (${res.status})`)
  return (await res.json()) as HealthResponse
}

export async function fetchStats(
  signal: AbortSignal,
): Promise<StatsResponse> {
  const res = await fetch(withPrefix("/api/stats"), {
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`stats failed (${res.status})`)
  return (await res.json()) as StatsResponse
}

export async function triggerReindex(
  signal: AbortSignal,
): Promise<ReindexResponse> {
  const res = await fetch(withPrefix("/api/reindex"), {
    method: "POST",
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`reindex failed (${res.status})`)
  return (await res.json()) as ReindexResponse
}

export async function fetchFileServer(
  signal: AbortSignal,
): Promise<FileServerConfig> {
  const res = await fetch(withPrefix("/api/file-server"), {
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`file-server failed (${res.status})`)
  return (await res.json()) as FileServerConfig
}

export async function fetchFeedback(
  signal: AbortSignal,
): Promise<FeedbackConfig> {
  const res = await fetch(withPrefix("/api/feedback"), {
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`feedback failed (${res.status})`)
  return (await res.json()) as FeedbackConfig
}

export async function fetchTrending(
  signal: AbortSignal,
): Promise<TrendingResponse> {
  const res = await fetch(withPrefix("/api/trending"), {
    signal,
    headers: { accept: "application/json" },
  })
  if (!res.ok) throw new Error(`trending failed (${res.status})`)
  return (await res.json()) as TrendingResponse
}

export async function deleteTrending(
  query: string,
  signal: AbortSignal,
): Promise<void> {
  const params = new URLSearchParams({ q: query })
  const res = await fetch(
    `${withPrefix("/api/trending")}?${params.toString()}`,
    {
      method: "DELETE",
      signal,
      headers: { accept: "application/json" },
    },
  )
  if (!res.ok) throw new Error(`delete trending failed (${res.status})`)
}

/// Build a file-browse URL by appending a result's relative path to the
/// file-server base. Encodes each path segment (preserves `/`) and adds a
/// trailing slash for directories so dufs/caddy/nginx show the listing.
export function buildBrowseUrl(
  base: string,
  relativePath: string,
  isDir: boolean,
): string {
  const cleanBase = base.replace(/\/+$/, "")
  const enc = relativePath
    .split("/")
    .map(encodeURIComponent)
    .join("/")
  return `${cleanBase}/${enc}${isDir ? "/" : ""}`
}
