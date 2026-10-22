import { useState } from 'react'
import { RotateCw } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/layout/PageHeader'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { useConfig } from '@/api/queries'
import { ConfigForm } from './ConfigForm'
import { SystemTab } from './SystemTab'

export function SettingsPage() {
  const q = useConfig()
  const [tab, setTab] = useState('general')
  return (
    <>
      <PageHeader title="Settings" description="Edit config.toml fields and check the system." />
      {q.data?.restart_required && (
        <Alert className="mb-4">
          <RotateCw aria-hidden />
          <AlertTitle>Restart required</AlertTitle>
          <AlertDescription>The saved settings differ from what the running supervisor loaded. Restart the supervisor to apply them.</AlertDescription>
        </Alert>
      )}
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList className="h-auto flex-wrap">
          <TabsTrigger value="general">General</TabsTrigger>
          <TabsTrigger value="notifications">Notifications</TabsTrigger>
          <TabsTrigger value="worktrees">Worktrees</TabsTrigger>
          <TabsTrigger value="routing">Routing</TabsTrigger>
          <TabsTrigger value="ui">UI</TabsTrigger>
          <TabsTrigger value="system">System</TabsTrigger>
        </TabsList>
        <div className="mt-4">
          {tab === 'system' ? null : q.isError && !q.data ? (
            <TabsContent value={tab}>
              <ErrorAlert error={q.error} title="Could not load settings" onRetry={() => void q.refetch()} />
            </TabsContent>
          ) : !q.data ? (
            <TabsContent value={tab}>
              <div role="status" aria-busy="true" aria-label="Loading" className="max-w-xl space-y-3">
                <Skeleton className="h-9 w-full" />
                <Skeleton className="h-9 w-full" />
                <Skeleton className="h-9 w-full" />
              </div>
            </TabsContent>
          ) : (
            <ConfigForm view={q.data} />
          )}
          <TabsContent value="system">
            <SystemTab />
          </TabsContent>
        </div>
      </Tabs>
    </>
  )
}
