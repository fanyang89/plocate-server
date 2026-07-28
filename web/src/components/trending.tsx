import { useState } from "react"
import { Flame, LoaderCircle, Trash2 } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { useTrending } from "@/hooks/use-trending"

type TrendingProps = {
  onPick: (query: string) => void
}

export function Trending({ onPick }: TrendingProps) {
  const { t } = useTranslation()
  const {
    items,
    loading,
    blockingQuery,
    blockError,
    blockQuery,
    resetBlockError,
  } = useTrending()
  const [candidate, setCandidate] = useState<string | null>(null)

  const openConfirmation = (query: string) => {
    resetBlockError()
    setCandidate(query)
  }

  const confirmBlock = async () => {
    if (!candidate) return
    if (await blockQuery(candidate)) setCandidate(null)
  }

  if (loading || items.length === 0) return null

  return (
    <>
      <section
        aria-label={t("trending.title")}
        className="flex flex-wrap items-center gap-1.5 text-sm"
      >
        <span className="inline-flex items-center gap-1 text-muted-foreground">
          <Flame className="size-3.5" aria-hidden />
          {t("trending.title")}
        </span>
        {items.map((it) => {
          const blocking = blockingQuery === it.query
          return (
            <span
              key={it.query}
              className="inline-flex max-w-[16rem] items-stretch overflow-hidden rounded-full border border-border bg-muted/50 text-xs text-foreground transition-colors hover:border-primary/40"
            >
              <button
                type="button"
                onClick={() => onPick(it.query)}
                disabled={blocking}
                className="min-w-0 truncate px-2.5 py-0.5 transition-colors hover:bg-primary/10 disabled:opacity-50"
                title={it.query}
              >
                {it.query}
              </button>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => openConfirmation(it.query)}
                    disabled={blocking}
                    aria-label={t("trending.blockAria", { query: it.query })}
                    className="flex w-6 shrink-0 items-center justify-center border-l border-border text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-50"
                  >
                    {blocking ? (
                      <LoaderCircle className="size-3 animate-spin" aria-hidden />
                    ) : (
                      <Trash2 className="size-3" aria-hidden />
                    )}
                  </button>
                </TooltipTrigger>
                <TooltipContent>{t("trending.block")}</TooltipContent>
              </Tooltip>
            </span>
          )
        })}
      </section>

      <Dialog
        open={candidate !== null}
        onOpenChange={(open) => {
          if (!open && blockingQuery === null) {
            setCandidate(null)
            resetBlockError()
          }
        }}
      >
        <DialogContent
          showCloseButton={blockingQuery === null}
          onEscapeKeyDown={(event) => {
            if (blockingQuery !== null) event.preventDefault()
          }}
          onPointerDownOutside={(event) => {
            if (blockingQuery !== null) event.preventDefault()
          }}
        >
          <DialogHeader>
            <DialogTitle>{t("trending.confirmTitle")}</DialogTitle>
            <DialogDescription className="break-words">
              {t("trending.confirmDescription", { query: candidate })}
            </DialogDescription>
          </DialogHeader>
          {blockError && (
            <p className="text-xs text-destructive" role="alert">
              {t("trending.blockFailed")}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              onClick={() => setCandidate(null)}
              disabled={blockingQuery !== null}
            >
              {t("trending.cancel")}
            </Button>
            <Button
              variant="destructive"
              onClick={confirmBlock}
              disabled={blockingQuery !== null}
            >
              {blockingQuery !== null && (
                <LoaderCircle className="size-4 animate-spin" />
              )}
              {blockingQuery !== null
                ? t("trending.blocking")
                : t("trending.confirm")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  )
}
