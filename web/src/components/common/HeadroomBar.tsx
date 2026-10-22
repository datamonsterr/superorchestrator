import { cn } from '@/lib/utils'
import { formatPercent } from '@/lib/format'
import { headroomLevel, type HeadroomLevel } from '@/lib/headroom'
import type { HeadroomStatus } from '@/api/types'

const FILL: Record<Exclude<HeadroomLevel, 'unknown'>, string> = {
  ok: 'bg-green-600 dark:bg-green-500',
  warn: 'bg-amber-500 dark:bg-amber-400',
  critical: 'bg-red-600 dark:bg-red-500',
}

interface Props {
  pct: number | null | undefined
  status?: HeadroomStatus
  label?: string
  /** ISO time the window resets; shown as the tooltip title. */
  resetsAt?: string | null
  className?: string
}

export function HeadroomBar({ pct, status, label, resetsAt, className }: Props) {
  const level = headroomLevel(pct, status)
  const resetsText = resetsAt ? `Resets ${new Date(resetsAt).toLocaleString()}` : ''
  const known = level !== 'unknown'
  const value = known ? Math.max(0, Math.min(100, pct as number)) : 0
  return (
    <div className={cn('flex items-center gap-2', className)} title={resetsAt ? resetsText : undefined}>
      {label && <span className="w-7 shrink-0 text-xs text-muted-foreground">{label}</span>}
      <div
        role="progressbar"
        aria-label={label ? `${label} headroom` : 'Headroom'}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={known ? Math.round(value) : undefined}
        aria-valuetext={known ? `${formatPercent(value)} remaining` : 'unknown'}
        data-level={level}
        className={cn(
          'relative h-2 w-24 overflow-hidden rounded-full bg-muted',
          !known && 'bg-[repeating-linear-gradient(45deg,transparent,transparent_3px,var(--border)_3px,var(--border)_6px)]',
        )}
      >
        {known && <div className={cn('h-full rounded-full', FILL[level])} style={{ width: `${value}%` }} />}
      </div>
      <span className="w-14 shrink-0 text-xs tabular-nums">{known ? formatPercent(value) : 'unknown'}</span>
      {resetsAt && <span className="sr-only">{resetsText}</span>}
    </div>
  )
}
