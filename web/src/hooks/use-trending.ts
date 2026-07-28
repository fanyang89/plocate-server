import { useEffect, useRef, useState } from "react"
import { deleteTrending, fetchTrending, type TrendingItem } from "@/api"
import { matchesTrendingRule, trendingKeywords } from "@/lib/trending"

type TrendingData = {
  items: TrendingItem[]
  loading: boolean
  blockingQuery: string | null
  blockError: boolean
}

export type TrendingState = TrendingData & {
  blockQuery: (query: string) => Promise<boolean>
  resetBlockError: () => void
}

const POLL_MS = 60_000

export function useTrending(): TrendingState {
  const [state, setState] = useState<TrendingData>({
    items: [],
    loading: true,
    blockingQuery: null,
    blockError: false,
  })
  const fetchAbortRef = useRef<AbortController | null>(null)
  const blockAbortRef = useRef<AbortController | null>(null)
  const blockedRulesRef = useRef(new Set<string>())

  useEffect(() => {
    let cancelled = false

    const tick = async () => {
      const ctrl = new AbortController()
      fetchAbortRef.current?.abort()
      fetchAbortRef.current = ctrl
      try {
        const data = await fetchTrending(ctrl.signal)
        if (!cancelled) {
          const blockedRules = Array.from(blockedRulesRef.current)
          setState((current) => ({
            ...current,
            items: data.items.filter(
              (item) =>
                !blockedRules.some((rule) =>
                  matchesTrendingRule(item.query, rule),
                ),
            ),
            loading: false,
          }))
        }
      } catch (e) {
        if ((e as Error).name === "AbortError") return
        // Silent: trending is a non-essential panel.
        if (!cancelled) setState((s) => ({ ...s, loading: false }))
      }
    }

    tick()
    const id = setInterval(tick, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
      fetchAbortRef.current?.abort()
      blockAbortRef.current?.abort()
    }
  }, [])

  const blockQuery = async (query: string) => {
    fetchAbortRef.current?.abort()
    const ctrl = new AbortController()
    blockAbortRef.current?.abort()
    blockAbortRef.current = ctrl
    setState((current) => ({
      ...current,
      blockingQuery: query,
      blockError: false,
    }))
    try {
      await deleteTrending(query, ctrl.signal)
      const rule = trendingKeywords(query).join(" ")
      blockedRulesRef.current.add(rule)
      setState((current) => ({
        ...current,
        items: current.items.filter(
          (item) => !matchesTrendingRule(item.query, rule),
        ),
        blockingQuery: null,
      }))
      return true
    } catch (e) {
      if ((e as Error).name === "AbortError") return false
      setState((current) => ({
        ...current,
        blockingQuery: null,
        blockError: true,
      }))
      return false
    } finally {
      if (blockAbortRef.current === ctrl) blockAbortRef.current = null
    }
  }

  const resetBlockError = () => {
    setState((current) => ({ ...current, blockError: false }))
  }

  return { ...state, blockQuery, resetBlockError }
}
