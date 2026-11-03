import { useState } from 'react'
import { ChevronDown, Lightbulb } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { Duration } from '@/components/common/Duration'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { RelativeTime } from '@/components/common/RelativeTime'
import { ScrollTable } from '@/components/common/ScrollTable'
import { useInsights, useLevels, usePutLevels } from '@/api/queries'
import type { InsightExample, Level, LevelInsight, Suggestion } from '@/api/types'

const pct = (v: number | null | undefined) => (v == null ? '-' : `${Math.round(v * 100)}%`)

/** Simple CSS bars: the height of each bar is its count relative to the fullest bucket. */
function Distribution({ level }: { level: LevelInsight }) {
  const max = Math.max(1, ...level.buckets.map((b) => b.count))
  return (
    <ul aria-label={`Complexity distribution of ${level.name}`} className="flex h-20 items-end gap-1">
      {level.buckets.map((b) => (
        <li key={b.from} className="flex h-full min-w-0 flex-1 flex-col justify-end gap-0.5 text-center" title={`${b.from}–${b.to}: ${b.count}`}>
          <span className="text-[10px] tabular-nums text-muted-foreground">{b.count}</span>
          <span className="block rounded-sm bg-primary/70" style={{ height: `${(b.count / max) * 100}%`, minHeight: b.count > 0 ? 2 : 0 }} />
          <span className="truncate text-[10px] tabular-nums text-muted-foreground">{b.from}</span>
        </li>
      ))}
    </ul>
  )
}

function Example({ ex }: { ex: InsightExample }) {
  return (
    <li className="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5 text-sm">
      <Badge variant={ex.source === 'history' ? 'secondary' : 'outline'}>{ex.source}</Badge>
      <span className="font-mono text-xs tabular-nums">{`c=${ex.complexity}`}</span>
      {ex.outcome && <Badge variant="outline">{ex.outcome}</Badge>}
      {ex.borderline && <Badge variant="outline">borderline</Badge>}
      <span className="min-w-0 flex-1 basis-48 break-words">{ex.excerpt}</span>
    </li>
  )
}

function LevelSection({ level }: { level: LevelInsight }) {
  const rates: [string, number | null | undefined][] = [
    ['success', level.success_rate],
    ['accepted', level.accepted_rate],
    ['rejected', level.rejected_rate],
    ['failed', level.failed_rate],
    ['failover', level.failover_rate],
    ['escalated', level.escalated_rate],
  ]
  const hasHistory = level.runs > 0
  return (
    <Collapsible defaultOpen role="region" aria-label={`${level.name} level insights`} className="rounded-md border">
      <CollapsibleTrigger asChild>
        <Button variant="ghost" className="flex h-auto w-full flex-wrap justify-start gap-2 px-3 py-2">
          <ChevronDown aria-hidden />
          <span className="font-semibold">{level.name}</span>
          <span className="text-sm font-normal tabular-nums text-muted-foreground">
            {level.min}–{level.max}
          </span>
          <span className="text-sm font-normal text-muted-foreground">
            {level.runs} {level.runs === 1 ? 'run' : 'runs'}
          </span>
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col gap-3 border-t p-3">
        {hasHistory ? (
          <>
            <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-sm sm:grid-cols-3">
              {rates.map(([label, v]) => (
                <div key={label} className="flex justify-between gap-2">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd className="tabular-nums">{pct(v)}</dd>
                </div>
              ))}
              <div className="flex justify-between gap-2">
                <dt className="text-muted-foreground">median complexity</dt>
                <dd className="tabular-nums">{level.complexity_median ?? '-'}</dd>
              </div>
              <div className="flex justify-between gap-2">
                <dt className="text-muted-foreground">complexity range</dt>
                <dd className="tabular-nums">
                  {level.complexity_min ?? '-'}–{level.complexity_max ?? '-'}
                </dd>
              </div>
              <div className="flex justify-between gap-2">
                <dt className="text-muted-foreground">median duration</dt>
                <dd className="tabular-nums">{level.median_duration_ms == null ? '-' : <Duration ms={level.median_duration_ms} />}</dd>
              </div>
            </dl>
            <Distribution level={level} />
            {level.models.length > 0 && (
              <ScrollTable>
                <Table aria-label={`Models of ${level.name} by outcome`}>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Model</TableHead>
                      <TableHead className="text-right">Runs</TableHead>
                      <TableHead className="text-right">Success</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {level.models.map((m, i) => (
                      <TableRow key={m.choice_id ?? i}>
                        <TableCell className="font-mono text-xs">{m.choice_id ?? 'unknown'}</TableCell>
                        <TableCell className="text-right tabular-nums">{m.runs}</TableCell>
                        <TableCell className="text-right tabular-nums">{pct(m.success_rate)}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </ScrollTable>
            )}
          </>
        ) : (
          <p className="text-sm text-muted-foreground">No history for this level yet. The examples below come from the eval set.</p>
        )}
        {level.examples.length > 0 && (
          <ul aria-label={`Example tasks of ${level.name}`} className="flex flex-col gap-1.5">
            {level.examples.map((ex, i) => (
              <Example key={ex.run_id ?? ex.eval_id ?? i} ex={ex} />
            ))}
          </ul>
        )}
      </CollapsibleContent>
    </Collapsible>
  )
}

function SuggestionCard({ s, onApply }: { s: Suggestion; onApply: (s: Suggestion) => void }) {
  return (
    <li className="flex flex-col gap-2 rounded-md border p-3" data-suggestion={s.id}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="secondary">{s.rule}</Badge>
        <span className="font-medium">{s.level}</span>
        <Button size="sm" variant="outline" className="ml-auto" aria-label={`Apply suggestion ${s.id}`} onClick={() => onApply(s)}>
          Apply
        </Button>
      </div>
      <p className="text-sm">{s.rationale}</p>
      <ul aria-label={`Changes of ${s.id}`} className="flex flex-col gap-1 font-mono text-xs break-all">
        {s.diff.map((d, i) => (
          <li key={i}>{`${d.level}.${d.field}: ${d.before} -> ${d.after}`}</li>
        ))}
      </ul>
    </li>
  )
}

const STALE = 'Levels changed since this suggestion was made; refresh insights'

/** True when a diff entry's `before` no longer matches the current levels (formatted as the server does). */
function isStale(s: Suggestion, current: Level[]): boolean {
  return s.diff.some((d) => {
    const l = current.find((x) => x.name === d.level)
    if (!l) return true
    if (d.field === 'min') return String(l.min) !== d.before
    if (d.field === 'max') return String(l.max) !== d.before
    if (d.field === 'models') return l.models.join(', ') !== d.before
    return false
  })
}

/** Level insights from run history and the eval set, with suggested level changes the user can apply. */
export function InsightsPanel() {
  const insights = useInsights()
  const levels = useLevels()
  const put = usePutLevels()
  const [target, setTarget] = useState<Suggestion | null>(null)
  const [error, setError] = useState<unknown>(null)
  const report = insights.data

  const apply = () => {
    if (put.isPending || !target || !levels.data) return
    const s = target
    const current = levels.data
    setError(null)
    if (isStale(s, current.levels)) {
      setError(new Error(STALE))
      setTarget(null)
      void insights.refetch()
      return
    }
    put
      .mutateAsync({ levels: s.levels.map((l) => ({ ...l, models: l.models ?? [] })), routing_instruction: current.routing_instruction })
      .then((view) => toast.success(`Applied suggestion for ${s.level}${view.restart_required ? '. Restart the supervisor for routing changes to take effect.' : ''}`))
      .catch(setError)
      .finally(() => setTarget(null))
  }

  return (
    <Card role="region" aria-label="Insights" className="min-w-0">
      <CardHeader>
        <CardTitle className="flex flex-wrap items-center gap-2 text-base">
          <Lightbulb aria-hidden className="size-4" />
          <span>Insights</span>
          {report && (
            <span className="text-sm font-normal text-muted-foreground">
              {report.history_runs} history {report.history_runs === 1 ? 'run' : 'runs'}, generated <RelativeTime value={report.generated_at} />
            </span>
          )}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {insights.isError && !report ? (
          <ErrorAlert error={insights.error} title="Could not load insights" onRetry={() => void insights.refetch()} />
        ) : !report ? (
          <Skeleton className="h-32" />
        ) : (
          <>
            {report.history_runs === 0 && <p className="text-sm text-muted-foreground">No run history yet. Showing the eval examples for each level.</p>}
            <div className="flex flex-col gap-2">
              {report.levels.map((l) => (
                <LevelSection key={l.name} level={l} />
              ))}
            </div>
            <section aria-label="Suggestions" className="flex flex-col gap-2">
              <h2 className="text-sm font-semibold">Suggestions</h2>
              {error !== null && <ErrorAlert error={error} title="Could not apply the suggestion" />}
              {report.suggestions.length === 0 ? (
                <p className="text-sm text-muted-foreground">No suggestions: the levels look consistent with the data.</p>
              ) : (
                <ul className="flex flex-col gap-2">
                  {report.suggestions.map((s) => (
                    <SuggestionCard key={s.id} s={s} onApply={(x) => (setError(null), setTarget(x))} />
                  ))}
                </ul>
              )}
            </section>
          </>
        )}
      </CardContent>
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => !o && setTarget(null)}
        title={`Apply suggestion for ${target?.level ?? ''}?`}
        description={
          levels.data
            ? `${target?.rationale ?? ''} This replaces the levels with the suggested set and keeps the routing instruction.`
            : 'The current levels could not be loaded, so the suggestion cannot be checked or applied. Reload the page and try again.'
        }
        confirmLabel="Apply suggestion"
        pending={put.isPending || !levels.data}
        onConfirm={apply}
      />
    </Card>
  )
}
