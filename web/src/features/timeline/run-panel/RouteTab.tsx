import { ProbabilityBars } from '@/components/common/ProbabilityBars'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { useRunRoute } from '@/api/queries'
import { formatMoney, formatPercent, formatTokens } from '@/lib/format'

/** The router decision: probabilities, source, confidence, rationale, alternatives and the decision's own usage. */
export function RouteTab({ runId }: { runId: string }) {
  const route = useRunRoute(runId)
  if (route.isPending) return <Skeleton className="h-40" />
  if (route.isError) return <ErrorAlert error={route.error} title="No route decision" onRetry={() => void route.refetch()} />
  const r = route.data
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Badge variant="secondary">{r.source}</Badge>
        <span className="font-medium">{r.choice_id}</span>
        <span className="text-muted-foreground">
          {r.harness} · {r.account} · {r.model}
          {r.effort ? ` · ${r.effort}` : ''}
        </span>
        {r.confidence != null && <span>Confidence {formatPercent(r.confidence * 100)}</span>}
      </div>
      <section aria-labelledby="prob-h">
        <h3 id="prob-h" className="mb-2 text-sm font-medium">
          Probabilities
        </h3>
        <ProbabilityBars probabilities={r.probabilities} selected={r.choice_id} />
      </section>
      <section aria-labelledby="why-h">
        <h3 id="why-h" className="mb-1 text-sm font-medium">
          Rationale
        </h3>
        <p className="text-sm whitespace-pre-wrap">{r.rationale || 'None given.'}</p>
      </section>
      {r.alternatives.length > 0 && (
        <section aria-labelledby="alt-h">
          <h3 id="alt-h" className="mb-1 text-sm font-medium">
            Alternatives
          </h3>
          <ul className="flex flex-col gap-1 text-sm">
            {r.alternatives.map((a) => (
              <li key={`${a.choice_id}-${a.account ?? ''}`} className="flex gap-2">
                <span className="font-medium">{a.choice_id}</span>
                {a.account && <span className="text-muted-foreground">{a.account}</span>}
                {a.score != null && <span className="ml-auto tabular-nums">score {a.score.toFixed(2)}</span>}
              </li>
            ))}
          </ul>
        </section>
      )}
      <p className="text-xs text-muted-foreground">
        Decision usage: {formatTokens(r.usage.input_tokens)} in, {formatTokens(r.usage.output_tokens)} out, {formatMoney(r.usage.cost)}
      </p>
    </div>
  )
}
