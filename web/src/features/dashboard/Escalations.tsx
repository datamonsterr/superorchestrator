import { Link } from 'react-router'
import { CheckCircle2, TriangleAlert } from 'lucide-react'
import { EmptyState } from '@/components/common/EmptyState'
import { RelativeTime } from '@/components/common/RelativeTime'
import type { Event } from '@/api/types'

/** Short human text from an event payload: the reason when it is a string, never the raw object. */
function escalationText(ev: Event): string {
  const d = ev.data as { reason?: unknown } | null
  if (d && typeof d.reason === 'string') return d.reason
  return ev.type === 'stall_detected' ? 'stalled' : 'needs attention'
}

export function Escalations({ events }: { events: Event[] }) {
  if (events.length === 0) return <EmptyState icon={CheckCircle2} title="No escalations" description="Nothing needs attention." />
  return (
    <ul className="flex max-h-72 flex-col divide-y overflow-y-auto rounded-md border" aria-label="Recent escalations">
      {events.map((ev) => (
        <li key={ev.id} className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-2 text-sm">
          <TriangleAlert aria-hidden className="size-4 shrink-0 text-red-600 dark:text-red-400" />
          {ev.run_id ? (
            <Link className="font-mono underline-offset-4 hover:underline" to={`/sessions/${ev.session_id}/runs/${ev.run_id}`}>
              {ev.run_id}
            </Link>
          ) : (
            <span className="font-mono">{ev.session_id}</span>
          )}
          <span>{escalationText(ev)}</span>
          <RelativeTime className="ml-auto text-xs text-muted-foreground" value={ev.ts} />
        </li>
      ))}
    </ul>
  )
}
