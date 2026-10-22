import { useStatus } from '@/api/queries'
import { capLevel } from '@/lib/cap'
import { cn } from '@/lib/utils'

export function CapMeterView({ active, cap }: { active: number; cap: number }) {
  const level = capLevel(active, cap)
  const pct = cap > 0 ? Math.min(100, (active / cap) * 100) : 0
  const fill = { ok: 'bg-green-600', warn: 'bg-amber-500', full: 'bg-red-600' }[level]
  return (
    <div className="flex items-center gap-2 text-sm" data-level={level}>
      <span className="text-muted-foreground">cap</span>
      <span className="tabular-nums">
        {active}/{cap}
      </span>
      <div
        role="progressbar"
        aria-label="Global cap usage"
        aria-valuemin={0}
        aria-valuemax={cap}
        aria-valuenow={Math.min(active, cap)}
        aria-valuetext={`${active} of ${cap} runs`}
        className="hidden h-2 w-16 overflow-hidden rounded-full bg-muted sm:block"
      >
        <div className={cn('h-full', fill)} style={{ width: `${pct}%` }} />
      </div>
    </div>
  )
}

/** Active runs against the global cap, both from GET /status. */
export function CapMeter() {
  const { data } = useStatus()
  if (!data) return null
  return <CapMeterView active={data.active_runs} cap={data.caps.global} />
}
