import { useState } from 'react'
import { RotateCw } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/layout/PageHeader'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { useConfig, useDecider } from '@/api/queries'
import { ConfigForm } from './ConfigForm'
import { DeciderForm } from './DeciderForm'
import { SystemTab } from './SystemTab'

export function SettingsPage() {
  const q = useConfig()
  const d = useDecider()
  const [tab, setTab] = useState('general')
  // The System and Decider tabs have their own content; the config form stays mounted behind them.
  const own = tab === 'system' || tab === 'decider'
  return (
    <>
      <PageHeader title="Settings" description="Edit config.toml fields and check the system." />
      {(q.data?.restart_required || d.data?.restart_required) && (
        <Alert className="mb-4">
          <RotateCw aria-hidden />
          <AlertTitle>Restart required</AlertTitle>
          <AlertDescription>The saved settings differ from what the running supervisor loaded. Restart the supervisor to apply them.</AlertDescription>
        </Alert>
      )}
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList className="max-w-full justify-start overflow-x-auto">
          <TabsTrigger value="general">General</TabsTrigger>
          <TabsTrigger value="notifications">Notifications</TabsTrigger>
          <TabsTrigger value="worktrees">Worktrees</TabsTrigger>
          <TabsTrigger value="decider">Decider</TabsTrigger>
          <TabsTrigger value="ui">UI</TabsTrigger>
          <TabsTrigger value="system">System</TabsTrigger>
        </TabsList>
        <div className="mt-4">
          {q.isError && !q.data ? (
            !own && (
              <TabsContent value={tab}>
                <ErrorAlert error={q.error} title="Could not load settings" onRetry={() => void q.refetch()} />
              </TabsContent>
            )
          ) : !q.data ? (
            !own && (
              <TabsContent value={tab}>
                <div role="status" aria-busy="true" aria-label="Loading" className="max-w-xl space-y-3">
                  <Skeleton className="h-9 w-full" />
                  <Skeleton className="h-9 w-full" />
                  <Skeleton className="h-9 w-full" />
                </div>
              </TabsContent>
            )
          ) : (
            // Stays mounted on the System tab so an unsaved draft survives a look at the doctor report.
            <ConfigForm view={q.data} hidden={own} />
          )}
          {d.data ? (
            <DeciderForm view={d.data} hidden={tab !== 'decider'} />
          ) : (
            tab === 'decider' && (
              <TabsContent value="decider">
                {d.isError ? (
                  <ErrorAlert error={d.error} title="Could not load the decider settings" onRetry={() => void d.refetch()} />
                ) : (
                  <div role="status" aria-busy="true" aria-label="Loading" className="max-w-xl space-y-3">
                    <Skeleton className="h-9 w-full" />
                    <Skeleton className="h-9 w-full" />
                    <Skeleton className="h-9 w-full" />
                  </div>
                )}
              </TabsContent>
            )
          )}
          <TabsContent value="system">
            <SystemTab />
          </TabsContent>
        </div>
      </Tabs>
    </>
  )
}
