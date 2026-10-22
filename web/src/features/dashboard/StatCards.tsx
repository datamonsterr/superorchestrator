import type { ReactNode } from 'react'
import { Card, CardContent } from '@/components/ui/card'
import { CapMeterView } from '@/components/layout/CapMeter'
import { formatMoney } from '@/lib/format'
import type { DashboardView } from '@/api/types'

function Stat({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <Card size="sm" className="gap-1">
      <CardContent className="flex flex-col gap-1">
        <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground">{label}</p>
        <div className="font-mono text-2xl font-semibold tabular-nums">{children}</div>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </CardContent>
    </Card>
  )
}

/** Today's cost is the newest day of the usage series. */
function todayCost(daily: DashboardView['daily']): number {
  return daily.length ? daily[daily.length - 1].cost : 0
}

export function StatCards({ data }: { data: DashboardView }) {
  const { status, caps, live_runs, escalations, daily } = data
  return (
    <section aria-label="Overview" className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <Stat label="Cap" hint="active runs against the global cap">
        <CapMeterView active={status.active_runs} cap={caps.global} />
      </Stat>
      <Stat label="Live runs" hint={`${status.sessions} sessions`}>
        {live_runs.length}
      </Stat>
      <Stat label="Escalations" hint={escalations.length ? 'need attention' : 'all clear'}>
        <span className={escalations.length ? 'text-red-600 dark:text-red-400' : undefined}>{escalations.length}</span>
      </Stat>
      <Stat label="Cost today">{formatMoney(todayCost(daily))}</Stat>
    </section>
  )
}
