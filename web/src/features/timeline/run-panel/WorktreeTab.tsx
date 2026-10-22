import type { RunViewPublic } from '@/api/types'
import { CopyButton } from '@/components/common/CopyButton'
import { Badge } from '@/components/ui/badge'
import { formatClock } from '@/lib/time'
import { briefFrom } from './data'
import { useAllRunEvents } from './hooks'

function Row({ label, value }: { label: string; value?: string }) {
  return (
    <div className="contents">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 items-center gap-2">
        {value ? (
          <>
            <code className="min-w-0 flex-1 font-mono text-xs break-all">{value}</code>
            <CopyButton value={value} label={`Copy ${label.toLowerCase()}`} />
          </>
        ) : (
          <span className="text-muted-foreground">none</span>
        )}
      </dd>
    </div>
  )
}

export function WorktreeTab({ run }: { run: RunViewPublic }) {
  const events = useAllRunEvents(run.run_id)
  const checks = (events.data ?? []).filter((e) => e.type === 'check_result')
  const isolation = briefFrom(events.data ?? [])?.isolation ?? (run.worktree ? 'worktree' : undefined)
  return (
    <div className="flex flex-col gap-4">
      <dl className="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 text-sm">
        <Row label="Path" value={run.worktree ?? undefined} />
        <Row label="Branch" value={run.branch ?? undefined} />
        <Row label="Cwd" value={run.cwd ?? undefined} />
        <div className="contents">
          <dt className="text-muted-foreground">Isolation</dt>
          <dd>{isolation ?? 'unknown'}</dd>
        </div>
      </dl>
      <section aria-labelledby="checks-h">
        <h3 id="checks-h" className="mb-1 text-sm font-medium">
          Checks
        </h3>
        {checks.length === 0 ? (
          <p className="text-sm text-muted-foreground">No checks have run.</p>
        ) : (
          <ul className="flex flex-col gap-1 text-sm">
            {checks.map((e) => {
              const d = (e.data ?? {}) as { name?: string; ok?: boolean }
              return (
                <li key={e.id} className="flex items-center gap-2">
                  <Badge variant={d.ok === false ? 'destructive' : 'secondary'}>{d.ok === false ? 'failed' : 'passed'}</Badge>
                  <span>{d.name ?? 'check'}</span>
                  <span className="ml-auto font-mono text-xs text-muted-foreground">{formatClock(Date.parse(e.ts), true)}</span>
                </li>
              )
            })}
          </ul>
        )}
      </section>
    </div>
  )
}
