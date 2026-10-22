import { useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { NotAvailable } from '@/components/common/NotAvailable'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { ScrollTable } from '@/components/common/ScrollTable'
import { isNotAvailable } from '@/lib/errors'
import { useDoctor, usePruneWorktrees, useWorktrees } from '@/api/queries'

const STATUS_LABEL = { ok: 'OK', warn: 'Warning', fail: 'Failed' } as const

function Doctor() {
  const q = useDoctor()
  return (
    <section aria-labelledby="doctor-h" className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <h2 id="doctor-h" className="text-lg font-semibold">
          Doctor
        </h2>
        <Button variant="outline" size="sm" onClick={() => void q.refetch()} disabled={q.isFetching}>
          {q.isFetching ? 'Checking...' : 'Run checks'}
        </Button>
      </div>
      {q.isError && !q.data ? (
        isNotAvailable(q.error) ? (
          <NotAvailable what="The doctor report" />
        ) : (
          <ErrorAlert error={q.error} title="Could not run the checks" onRetry={() => void q.refetch()} />
        )
      ) : q.isPending ? (
        <TableSkeleton rows={3} columns={3} />
      ) : q.data.checks.length === 0 ? (
        <EmptyState title="No checks reported" />
      ) : (
        <ScrollTable>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Check</TableHead>
                <TableHead scope="col">Result</TableHead>
                <TableHead scope="col">Detail</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {q.data.checks.map((c) => (
                <TableRow key={c.name}>
                  <TableCell className="font-medium">{c.name}</TableCell>
                  <TableCell>
                    <Badge variant={c.status === 'fail' ? 'destructive' : c.status === 'warn' ? 'outline' : 'secondary'}>
                      {STATUS_LABEL[c.status] ?? c.status}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">{c.detail}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </ScrollTable>
      )}
    </section>
  )
}

const mb = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`

function Worktrees() {
  const q = useWorktrees()
  const prune = usePruneWorktrees()
  const [plan, setPlan] = useState<string[] | null>(null)
  const pathOf = (runId: string) => q.data?.find((w) => w.run_id === runId)?.path

  function dryRun() {
    prune.mutate(
      { dry_run: true },
      {
        onSuccess: (r) => (r.removed.length === 0 ? toast('Nothing to prune') : setPlan(r.removed)),
      },
    )
  }
  function confirm() {
    // Only the runs the user saw and confirmed: one that became busy since the dry run is skipped.
    prune.mutate(
      { dry_run: false, ids: plan ?? [] },
      {
        onSuccess: (r) => {
          const n = r.removed.length
          const skipped = r.skipped.length > 0 ? `, skipped ${r.skipped.length}` : ''
          toast.success(`Pruned ${n} run${n === 1 ? '' : 's'}${skipped}`)
        },
        onSettled: () => setPlan(null),
      },
    )
  }

  return (
    <section aria-labelledby="wt-h" className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <h2 id="wt-h" className="text-lg font-semibold">
          Worktrees
        </h2>
        <Button variant="outline" size="sm" onClick={dryRun} disabled={prune.isPending}>
          Prune orphaned
        </Button>
      </div>
      {q.isError && !q.data ? (
        isNotAvailable(q.error) ? (
          <NotAvailable what="The worktree list" />
        ) : (
          <ErrorAlert error={q.error} title="Could not load worktrees" onRetry={() => void q.refetch()} />
        )
      ) : q.isPending ? (
        <TableSkeleton rows={3} columns={4} />
      ) : q.data.length === 0 ? (
        <EmptyState title="No worktrees" description="Runs with worktree isolation create one each." />
      ) : (
        <ScrollTable>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Path</TableHead>
                <TableHead scope="col">Branch</TableHead>
                <TableHead scope="col">Run</TableHead>
                <TableHead scope="col" className="text-right">
                  Size
                </TableHead>
                <TableHead scope="col">State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {q.data.map((w) => (
                <TableRow key={w.path}>
                  <TableCell className="max-w-xs truncate font-mono text-xs" title={w.path}>
                    {w.path}
                  </TableCell>
                  <TableCell className="font-mono text-xs">{w.branch ?? '-'}</TableCell>
                  <TableCell className="font-mono text-xs">{w.run_id}</TableCell>
                  <TableCell className="text-right tabular-nums">{mb(w.size_bytes)}</TableCell>
                  <TableCell>
                    <Badge variant={w.orphaned ? 'outline' : 'secondary'}>{w.orphaned ? 'Orphaned' : 'In use'}</Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </ScrollTable>
      )}
      <ConfirmDialog
        open={plan !== null}
        onOpenChange={(o) => !o && setPlan(null)}
        title={`Prune the worktrees of ${plan?.length ?? 0} run${plan?.length === 1 ? '' : 's'}?`}
        description={
          <>
            The worktree folders and branches of these finished runs will be deleted from disk:
            <span className="mt-2 block space-y-0.5 font-mono text-xs">
              {plan?.map((id) => (
                <span key={id} className="block break-all">
                  {id}
                  {pathOf(id) ? ` (${pathOf(id)})` : ''}
                </span>
              ))}
            </span>
          </>
        }
        confirmLabel="Prune"
        destructive
        pending={prune.isPending}
        onConfirm={confirm}
      />
    </section>
  )
}

export function SystemTab() {
  return (
    <div className="space-y-8">
      <Doctor />
      <Worktrees />
    </div>
  )
}
