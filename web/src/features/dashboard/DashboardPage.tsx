import { useCallback } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { PageHeader } from '@/components/layout/PageHeader'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { useDailyUsage, useDashboard } from '@/api/queries'
import type { RunViewPublic } from '@/api/types'
import { StatCards } from './StatCards'
import { AccountsSummaryTable } from './AccountsSummaryTable'
import { LiveRuns } from './LiveRuns'
import { Escalations } from './Escalations'
import { UsageChart } from './UsageChart'

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm font-medium uppercase tracking-wider text-muted-foreground">{title}</CardTitle>
      </CardHeader>
      <CardContent className="overflow-x-auto">{children}</CardContent>
    </Card>
  )
}

export function DashboardPage() {
  const dash = useDashboard()
  const usage = useDailyUsage(30)
  const data = dash.data
  // TODO(T2.1): use run.session_id once RunViewPublic carries it.
  // RunViewPublic has no session id: take it from the run's recent events or an escalation about it.
  const sessionOf = useCallback(
    (run: RunViewPublic) =>
      run.recent_events[0]?.session_id ?? data?.escalations.find((e) => e.run_id === run.run_id)?.session_id,
    [data],
  )

  return (
    <>
      <PageHeader title="Dashboard" description="Capacity, live runs and escalations." />
      {dash.isError && !data ? (
        <ErrorAlert error={dash.error} title="Could not load the dashboard" onRetry={() => void dash.refetch()} />
      ) : !data ? (
        <div className="flex flex-col gap-4" data-testid="dashboard-loading">
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            {Array.from({ length: 4 }, (_, i) => (
              <Skeleton key={i} className="h-24" />
            ))}
          </div>
          <TableSkeleton rows={3} columns={6} />
        </div>
      ) : (
        <div className="flex flex-col gap-4">
          {dash.isError && <ErrorAlert error={dash.error} title="Showing stale data" onRetry={() => void dash.refetch()} />}
          <StatCards data={data} />
          <Section title="Accounts">
            <AccountsSummaryTable accounts={data.accounts} />
          </Section>
          <div className="grid gap-4 lg:grid-cols-2">
            <Section title="Live runs">
              <LiveRuns runs={data.live_runs} sessionOf={sessionOf} />
            </Section>
            <Section title="Recent escalations">
              <Escalations events={data.escalations} />
            </Section>
          </div>
          <Section title="Cost and tokens per day">
            {usage.isError ? (
              <ErrorAlert error={usage.error} title="Could not load usage" onRetry={() => void usage.refetch()} />
            ) : usage.data ? (
              <UsageChart daily={usage.data} />
            ) : (
              <Skeleton className="h-56" />
            )}
          </Section>
        </div>
      )}
    </>
  )
}
