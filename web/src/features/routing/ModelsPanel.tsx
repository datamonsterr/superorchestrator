import { RefreshCw } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Skeleton } from '@/components/ui/skeleton'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { RelativeTime } from '@/components/common/RelativeTime'
import { useModels, useRefreshModels } from '@/api/queries'
import type { ModelEntry } from '@/api/types'

export function ModelsPanel() {
  const models = useModels()
  const refresh = useRefreshModels()
  const byHarness = new Map<string, ModelEntry[]>()
  for (const m of models.data?.models ?? []) byHarness.set(m.harness, [...(byHarness.get(m.harness) ?? []), m])

  return (
    <Collapsible className="rounded-md border">
      <div className="flex flex-wrap items-center justify-between gap-2 p-3">
        <div className="flex items-center gap-2">
          <CollapsibleTrigger asChild>
            <Button variant="ghost" size="sm">
              Models ({models.data?.models.length ?? 0})
            </Button>
          </CollapsibleTrigger>
          <span className="text-xs text-muted-foreground">
            {models.data?.refreshed_at ? (
              <>
                refreshed <RelativeTime value={models.data.refreshed_at} />
              </>
            ) : (
              'never refreshed'
            )}
          </span>
        </div>
        <Button variant="outline" size="sm" disabled={refresh.isPending} onClick={() => refresh.mutate()}>
          <RefreshCw aria-hidden className={refresh.isPending ? 'animate-spin' : undefined} />
          {refresh.isPending ? 'Refreshing…' : 'Refresh models'}
        </Button>
      </div>
      <CollapsibleContent className="border-t p-3">
        {models.isError ? (
          <ErrorAlert error={models.error} title="Could not load models" onRetry={() => void models.refetch()} />
        ) : models.isPending ? (
          <Skeleton className="h-16" />
        ) : byHarness.size === 0 ? (
          <p className="text-sm text-muted-foreground">No models known yet. Refresh to ask each harness.</p>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2">
            {[...byHarness].map(([harness, list]) => (
              <section key={harness} aria-label={`${harness} models`} className="flex flex-col gap-1.5">
                <HarnessBadge harness={list[0].harness} className="w-fit" />
                <ul className="flex flex-col gap-1 text-sm">
                  {list.map((m) => (
                    <li key={m.id} className="flex flex-wrap items-center gap-1.5">
                      <span className="font-mono text-xs">{m.id}</span>
                      {m.efforts.map((e) => (
                        <Badge key={e} variant="outline">
                          {e}
                        </Badge>
                      ))}
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
      </CollapsibleContent>
    </Collapsible>
  )
}
