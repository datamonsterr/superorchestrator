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
        <div className="overflow-x-auto rounded-md border">
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
        </div>
      )}
    </section>
  )
}

const mb = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`

function Worktrees() {
  const q = useWorktrees()
  const prune = usePruneWorktrees()
  const [plan, setPlan] = useState<string[] | null>(null)

  function dryRun() {
    prune.mutate(
      { dry_run: true },
      {
        onSuccess: (r) => (r.removed.length === 0 ? toast('Nothing to prune') : setPlan(r.removed)),
      },
    )
  }
  function confirm() {
    prune.mutate(
      { dry_run: false },
      {
        onSuccess: (r) => toast.success(`Pruned ${r.removed.length} worktree${r.removed.length === 1 ? '' : 's'}`),
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
        <div className="overflow-x-auto rounded-md border">
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
        </div>
      )}
      <ConfirmDialog
        open={plan !== null}
        onOpenChange={(o) => !o && setPlan(null)}
        title={`Prune ${plan?.length ?? 0} worktree${plan?.length === 1 ? '' : 's'}?`}
        description={
          <>
            The worktrees of these finished runs will be deleted from disk:{' '}
            <span className="font-mono">{plan?.join(', ')}</span>
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
