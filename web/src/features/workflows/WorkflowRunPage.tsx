import { useState } from 'react'
import { Link, useParams } from 'react-router'
import { ArrowLeft, RotateCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { PageHeader } from '@/components/layout/PageHeader'
import { Duration } from '@/components/common/Duration'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { JsonView } from '@/components/common/JsonView'
import { NotAvailable } from '@/components/common/NotAvailable'
import { ScrollTable } from '@/components/common/ScrollTable'
import { isNotAvailable } from '@/lib/errors'
import { useWorkflowRun } from '@/api/queries'
import { RunWorkflowDialog } from './RunWorkflowDialog'
import { WorkflowStateBadge } from './WorkflowStateBadge'

export function WorkflowRunPage() {
  const { id = '' } = useParams()
  const q = useWorkflowRun(id)
  const [again, setAgain] = useState(false)
  const run = q.data
  return (
    <>
      <PageHeader
        title={run ? `${run.name} run` : 'Workflow run'}
        description={<span className="font-mono text-xs">{id}</span>}
        actions={
          <>
            <Button variant="ghost" asChild>
              <Link to="/workflows">
                <ArrowLeft aria-hidden /> Workflows
              </Link>
            </Button>
            {run && (
              <Button variant="outline" onClick={() => setAgain(true)}>
                <RotateCw aria-hidden /> Run again
              </Button>
            )}
          </>
        }
      />
      {q.isError ? (
        isNotAvailable(q.error) ? (
          <NotAvailable what="Workflow runs" />
        ) : (
          <ErrorAlert error={q.error} title="Could not load the workflow run" onRetry={() => void q.refetch()} />
        )
      ) : !run ? (
        <div role="status" aria-busy="true" aria-label="Loading" className="space-y-2">
          <Skeleton className="h-6 w-48" />
          <Skeleton className="h-32 w-full" />
        </div>
      ) : (
        <div className="space-y-4">
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <WorkflowStateBadge state={run.state} />
            <Duration start={run.started_at} end={run.finished_at} />
            {run.session_id && (
              <Link to={`/sessions/${run.session_id}`} className="underline-offset-4 hover:underline">
                Session {run.session_id}
              </Link>
            )}
          </div>
          {run.error && <ErrorAlert error={new Error(run.error)} title="The workflow failed" />}
          {run.steps.length === 0 ? (
            <EmptyState title="No steps yet" description="Steps appear as the workflow starts them." />
          ) : (
            <ScrollTable>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Step</TableHead>
                    <TableHead scope="col">State</TableHead>
                    <TableHead scope="col">Run</TableHead>
                    <TableHead scope="col">Duration</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {run.steps.map((s) => (
                    <TableRow key={s.name}>
                      <TableCell className="font-medium">{s.name}</TableCell>
                      <TableCell>
                        <WorkflowStateBadge state={s.state} />
                      </TableCell>
                      <TableCell className="font-mono text-xs">
                        {s.run_id && run.session_id ? (
                          <Link to={`/sessions/${run.session_id}/runs/${s.run_id}`} className="underline-offset-4 hover:underline">
                            {s.run_id}
                          </Link>
                        ) : (
                          (s.run_id ?? '-')
                        )}
                      </TableCell>
                      <TableCell>{s.started_at ? <Duration start={s.started_at} end={s.finished_at} /> : '-'}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </ScrollTable>
          )}
          {run.result !== undefined && <JsonView value={run.result} label="Result" />}
        </div>
      )}
      <RunWorkflowDialog name={again && run ? run.name : null} onClose={() => setAgain(false)} />
    </>
  )
}
