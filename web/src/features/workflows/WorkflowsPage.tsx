import { useState } from 'react'
import { Link } from 'react-router'
import { GitBranch, Play } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/layout/PageHeader'
import { Duration } from '@/components/common/Duration'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { NotAvailable } from '@/components/common/NotAvailable'
import { RelativeTime } from '@/components/common/RelativeTime'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { ScrollTable } from '@/components/common/ScrollTable'
import { isNotAvailable } from '@/lib/errors'
import { useWorkflowRuns, useWorkflows } from '@/api/queries'
import { RunWorkflowDialog } from './RunWorkflowDialog'
import { WorkflowStateBadge } from './WorkflowStateBadge'

function Saved({ onRun }: { onRun: (name: string) => void }) {
  const q = useWorkflows()
  if (q.isError && !q.data) {
    return isNotAvailable(q.error) ? (
      <NotAvailable what="Workflows" />
    ) : (
      <ErrorAlert error={q.error} title="Could not load workflows" onRetry={() => void q.refetch()} />
    )
  }
  if (q.isPending) return <TableSkeleton rows={3} columns={4} />
  if (q.data.length === 0) {
    return <EmptyState icon={GitBranch} title="No saved workflows" description="Save a workflow script in the project or config workflows folder and it shows up here." />
  }
  return (
    <ScrollTable>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">Name</TableHead>
            <TableHead scope="col">Source</TableHead>
            <TableHead scope="col">Path</TableHead>
            <TableHead scope="col">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {q.data.map((w) => (
            <TableRow key={w.name}>
              <TableCell className="font-medium">{w.name}</TableCell>
              <TableCell>
                <Badge variant="secondary">{w.source}</Badge>
              </TableCell>
              <TableCell className="max-w-sm truncate font-mono text-xs text-muted-foreground" title={w.path}>
                {w.path}
              </TableCell>
              <TableCell className="text-right">
                <Button size="sm" variant="outline" onClick={() => onRun(w.name)} aria-label={`Run ${w.name}`}>
                  <Play aria-hidden /> Run
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </ScrollTable>
  )
}

function Runs() {
  const q = useWorkflowRuns()
  if (q.isError && !q.data) {
    return isNotAvailable(q.error) ? (
      <NotAvailable what="Workflow run history" />
    ) : (
      <ErrorAlert error={q.error} title="Could not load workflow runs" onRetry={() => void q.refetch()} />
    )
  }
  if (q.isPending) return <TableSkeleton rows={4} columns={5} />
  const runs = q.data.pages.flatMap((p) => p.items)
  if (runs.length === 0) return <EmptyState icon={GitBranch} title="No workflow runs yet" description="Run a saved workflow and its steps are listed here." />
  return (
    <div className="space-y-3">
      <ScrollTable>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">Run</TableHead>
              <TableHead scope="col">Workflow</TableHead>
              <TableHead scope="col">State</TableHead>
              <TableHead scope="col">Started</TableHead>
              <TableHead scope="col">Duration</TableHead>
              <TableHead scope="col">Session</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {runs.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="font-mono text-xs">
                  <Link to={`/workflows/${encodeURIComponent(r.id)}`} className="underline-offset-4 hover:underline">
                    {r.id}
                  </Link>
                </TableCell>
                <TableCell>{r.name}</TableCell>
                <TableCell>
                  <WorkflowStateBadge state={r.state} />
                </TableCell>
                <TableCell>
                  <RelativeTime value={r.started_at} />
                </TableCell>
                <TableCell>
                  <Duration start={r.started_at} end={r.finished_at} />
                </TableCell>
                <TableCell>
                  {r.session_id ? (
                    <Link to={`/sessions/${r.session_id}`} className="font-mono text-xs underline-offset-4 hover:underline">
                      {r.session_id}
                    </Link>
                  ) : (
                    '-'
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ScrollTable>
      {q.hasNextPage && (
        <Button variant="outline" onClick={() => void q.fetchNextPage()} disabled={q.isFetchingNextPage}>
          {q.isFetchingNextPage ? 'Loading...' : 'Load more'}
        </Button>
      )}
    </div>
  )
}

export function WorkflowsPage() {
  const [running, setRunning] = useState<string | null>(null)
  return (
    <>
      <PageHeader title="Workflows" description="Saved workflows and their runs." />
      <Tabs defaultValue="saved">
        <TabsList>
          <TabsTrigger value="saved">Saved</TabsTrigger>
          <TabsTrigger value="runs">Runs</TabsTrigger>
        </TabsList>
        <TabsContent value="saved" className="mt-4">
          <Saved onRun={setRunning} />
        </TabsContent>
        <TabsContent value="runs" className="mt-4">
          <Runs />
        </TabsContent>
      </Tabs>
      <RunWorkflowDialog name={running} onClose={() => setRunning(null)} />
    </>
  )
}
