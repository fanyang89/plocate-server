import { useEffect, useState } from "react"
import { ArrowUpRight } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Footer } from "@/components/footer"
import { HeaderActions } from "@/components/header-actions"
import { Results } from "@/components/results"
import { SearchBar } from "@/components/search-bar"
import { Trending } from "@/components/trending"
import { useDebounce } from "@/hooks/use-debounce"
import { useFileServer } from "@/hooks/use-file-server"
import { useHealth } from "@/hooks/use-health"
import { useSearch } from "@/hooks/use-search"

function Brand({
  instanceName,
  fileServerUrl,
  brandFallback,
}: {
  instanceName: string | null
  fileServerUrl: string | null
  brandFallback: string
}) {
  const name = instanceName ?? brandFallback
  if (instanceName && fileServerUrl) {
    return (
      <h1 className="text-lg font-semibold tracking-tight">
        <a
          href={fileServerUrl}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-1 text-foreground transition-colors hover:text-primary"
        >
          {name}
          <ArrowUpRight className="size-3.5 text-muted-foreground" />
        </a>
      </h1>
    )
  }
  return (
    <h1 className="text-lg font-semibold tracking-tight">{name}</h1>
  )
}

export default function App() {
  const { t, i18n: i18nInstance } = useTranslation()
  const [query, setQuery] = useState("")

  const debounced = useDebounce(query, 300)
  const stableQuery = useDebounce(query, 2_000)
  const { state, refetch } = useSearch(debounced, stableQuery)
  const health = useHealth()
  const fileServer = useFileServer()

  const instanceName = health.data?.instance_name ?? null
  const effectiveName = instanceName ?? t("app.brand")

  useEffect(() => {
    document.title = `${effectiveName} · ${t("app.titleSuffix")}`
  }, [effectiveName, t, i18nInstance.resolvedLanguage])

  return (
    <div className="mx-auto flex min-h-svh max-w-3xl flex-col px-4 py-8">
      <header className="mb-6 flex items-center justify-between">
        <Brand
          instanceName={instanceName}
          fileServerUrl={fileServer.url}
          brandFallback={t("app.brand")}
        />
        <HeaderActions
          instanceName={health.data?.instance_name ?? "plocate"}
          basePath={health.data?.base_path ?? null}
        />
      </header>

      <main className="flex flex-1 flex-col gap-4">
        <SearchBar value={query} onChange={setQuery} />
        <Trending onPick={setQuery} />
        <div className="flex-1">
          <Results
            state={state}
            onRetry={refetch}
            fileServerUrl={fileServer.url}
          />
        </div>
      </main>

      <Footer health={health} />
    </div>
  )
}
