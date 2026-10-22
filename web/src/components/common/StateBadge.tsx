import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import type { RunState } from '@/api/types'

const STYLE: Record<RunState, { label: string; dot: string; glyph: string }> = {
  queued: { label: 'Queued', dot: 'bg-slate-400', glyph: '…' },
  starting: { label: 'Starting', dot: 'bg-sky-500', glyph: '◌' },
  running: { label: 'Running', dot: 'bg-green-600 dark:bg-green-500', glyph: '▶' },
  waiting: { label: 'Waiting', dot: 'bg-blue-600 dark:bg-blue-400', glyph: '⏸' },
  stalled: { label: 'Stalled', dot: 'bg-amber-600 dark:bg-amber-400', glyph: '!' },
  escalated: { label: 'Escalated', dot: 'bg-red-600 dark:bg-red-400', glyph: '▲' },
  pending_review: { label: 'Pending review', dot: 'bg-purple-600 dark:bg-purple-400', glyph: '?' },
  succeeded: { label: 'Succeeded', dot: 'bg-teal-700 dark:bg-teal-400', glyph: '✓' },
  failed: { label: 'Failed', dot: 'bg-red-800 dark:bg-red-500', glyph: '✗' },
  cancelled: { label: 'Cancelled', dot: 'bg-zinc-500', glyph: '–' },
}

/** Text label plus a glyph, so state is never colour-only. */
export function StateBadge({ state, className }: { state: RunState; className?: string }) {
  const s = STYLE[state]
  return (
    <Badge variant="outline" className={cn('gap-1.5', className)} data-state={state}>
      <span aria-hidden className={cn('size-2 shrink-0 rounded-full', s.dot)} />
      <span aria-hidden className="font-mono text-[10px] leading-none">
        {s.glyph}
      </span>
      {s.label}
    </Badge>
  )
}
