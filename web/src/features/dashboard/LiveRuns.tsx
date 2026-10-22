import { Link } from 'react-router'
import { Activity } from 'lucide-react'
import { EmptyState } from '@/components/common/EmptyState'
import { StateBadge } from '@/components/common/StateBadge'
import { Duration } from '@/components/common/Duration'
import type { RunViewPublic } from '@/api/types'

export function LiveRuns({ runs }: { runs: RunViewPublic[] }) {
  if (runs.length === 0) return <EmptyState icon={Activity} title="No active runs" />
  return (
    <ul className="flex max-h-72 flex-col divide-y overflow-y-auto rounded-md border" aria-label="Live runs">
      {runs.map((r) => {
        const sid = r.session_id
        const label = r.name ?? r.run_id
        return (
          <li key={r.run_id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm">
            {sid ? (
              <Link className="font-medium underline-offset-4 hover:underline" to={`/sessions/${sid}/runs/${r.run_id}`}>
                {label}
              </Link>
            ) : (
              <span className="font-medium">{label}</span>
            )}
            <StateBadge state={r.state} />
            <span className="text-muted-foreground">{r.account ?? 'unassigned'}</span>
            <span className="ml-auto text-muted-foreground">
              <Duration start={r.created_at} end={r.finished_at} />
            </span>
          </li>
        )
      })}
    </ul>
  )
}
