import { useCallback, useEffect, useRef, useState } from "react"
import i18n from "@/i18n"
import { fetchSearch, recordTrending, type SearchResponse } from "@/api"

export type SearchStatus =
  | "idle"
  | "loading"
  | "error"
  | "empty"
  | "success"

export type SearchState = {
  status: SearchStatus
  data: SearchResponse | null
  error: string
  query: string
}

const INITIAL: SearchState = {
  status: "idle",
  data: null,
  error: "",
  query: "",
}

export function useSearch(debouncedQuery: string, stableQuery: string) {
  const [state, setState] = useState<SearchState>(INITIAL)
  const [nonce, setNonce] = useState(0)
  const abortRef = useRef<AbortController | null>(null)

  const refetch = useCallback(() => setNonce((n) => n + 1), [])

  useEffect(() => {
    const q = debouncedQuery.trim()
    if (!q) {
      abortRef.current?.abort()
      setState(INITIAL)
      return
    }

    const ctrl = new AbortController()
    abortRef.current?.abort()
    abortRef.current = ctrl

    setState((s) => ({
      status: "loading",
      data: s.data,
      error: "",
      query: q,
    }))

    let cancelled = false
    fetchSearch(q, ctrl.signal, false)
      .then((data) => {
        if (cancelled) return
        setState({
          status: data.items.length === 0 ? "empty" : "success",
          data,
          error: "",
          query: q,
        })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        if ((e as Error).name === "AbortError") return
        const msg =
          e instanceof Error ? e.message : i18n.t("errors.searchUnknown")
        setState((s) => ({
          status: "error",
          data: s.data,
          error: msg,
          query: q,
        }))
      })

    return () => {
      cancelled = true
      ctrl.abort()
    }
  }, [debouncedQuery, nonce])

  useEffect(() => {
    const q = stableQuery.trim()
    const searchCompleted = state.status === "success" || state.status === "empty"
    if (!q || !searchCompleted || state.query !== q) return

    const ctrl = new AbortController()
    recordTrending(q, ctrl.signal).catch((e: unknown) => {
      if ((e as Error).name === "AbortError") return
      // Silent: trending is non-essential and must not affect search results.
    })
    return () => ctrl.abort()
  }, [stableQuery, state.query, state.status])

  return { state, refetch }
}
