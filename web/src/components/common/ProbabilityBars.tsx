import { cn } from '@/lib/utils'

interface Props {
  probabilities: Record<string, number>
  /** Choice id to highlight. */
  selected?: string
  className?: string
  /** Accessible name of the list. */
  ariaLabel?: string
}

/** Bars sorted by probability, descending. The percent is always printed. */
export function ProbabilityBars({ probabilities, selected, className, ariaLabel = 'Choice probabilities' }: Props) {
  const rows = Object.entries(probabilities).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
  if (rows.length === 0) return <p className="text-sm text-muted-foreground">No probabilities</p>
  return (
    <ul className={cn('flex flex-col gap-1.5', className)} aria-label={ariaLabel}>
      {rows.map(([id, p]) => {
        const pct = Math.max(0, Math.min(1, p)) * 100
        const isSel = id === selected
        return (
          <li key={id} className="flex items-center gap-2 text-sm" data-selected={isSel || undefined}>
            <span className={cn('w-32 shrink-0 truncate', isSel && 'font-semibold')} title={id}>
              {id}
            </span>
            <div className="h-2 flex-1 overflow-hidden rounded-full bg-muted">
              <div className={cn('h-full rounded-full', isSel ? 'bg-primary' : 'bg-muted-foreground/50')} style={{ width: `${pct}%` }} />
            </div>
            <span className="w-12 shrink-0 text-right tabular-nums">{pct.toFixed(0)}%</span>
            {isSel && <span className="sr-only">selected</span>}
          </li>
        )
      })}
    </ul>
  )
}
