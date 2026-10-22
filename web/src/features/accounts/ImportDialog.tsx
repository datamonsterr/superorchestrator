import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Check, Loader2, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { qk, useAccountScan, useImportAccount } from '@/api/queries'
import { errorMessage } from '@/lib/errors'
import { FolderSearch } from 'lucide-react'

type RowState = { kind: 'pending' } | { kind: 'ok' } | { kind: 'error'; message: string }

export function ImportDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const qc = useQueryClient()
  const scan = useAccountScan(open)
  const importAccount = useImportAccount()
  const [picked, setPicked] = useState<Set<string>>(new Set())
  const [status, setStatus] = useState<Record<string, RowState>>({})
  const [running, setRunning] = useState(false)
  const candidates = scan.data ?? []

  const toggle = (name: string, on: boolean) =>
    setPicked((p) => {
      const n = new Set(p)
      if (on) n.add(name)
      else n.delete(name)
      return n
    })

  async function run() {
    setRunning(true)
    for (const c of candidates.filter((c) => picked.has(c.name) && status[c.name]?.kind !== 'ok')) {
      setStatus((s) => ({ ...s, [c.name]: { kind: 'pending' } }))
      try {
        await importAccount.mutateAsync({ name: c.name, harness: c.harness, home: c.home })
        setStatus((s) => ({ ...s, [c.name]: { kind: 'ok' } }))
      } catch (e) {
        setStatus((s) => ({ ...s, [c.name]: { kind: 'error', message: errorMessage(e) } }))
      }
    }
    setRunning(false)
    void qc.invalidateQueries({ queryKey: qk.accountsScan() })
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) {
          setPicked(new Set())
          setStatus({})
        }
        onOpenChange(o)
      }}
    >
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Scan and import</DialogTitle>
          <DialogDescription>Existing credential homes found on this machine. Only paths are listed.</DialogDescription>
        </DialogHeader>
        {scan.isError ? (
          <ErrorAlert error={scan.error} title="Scan failed" onRetry={() => void scan.refetch()} />
        ) : scan.isPending ? (
          <TableSkeleton rows={3} columns={4} />
        ) : candidates.length === 0 ? (
          <EmptyState icon={FolderSearch} title="Nothing to import" description="No unregistered credential homes were found." />
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-8">
                  <span className="sr-only">Import</span>
                </TableHead>
                <TableHead>Name</TableHead>
                <TableHead>Harness</TableHead>
                <TableHead>Home</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {candidates.map((c) => {
                const st = status[c.name]
                return (
                  <TableRow key={c.name}>
                    <TableCell>
                      <Checkbox
                        aria-label={`Import ${c.name}`}
                        checked={picked.has(c.name)}
                        disabled={running || st?.kind === 'ok'}
                        onCheckedChange={(v) => toggle(c.name, v === true)}
                      />
                    </TableCell>
                    <TableCell className="font-medium">{c.name}</TableCell>
                    <TableCell>
                      <HarnessBadge harness={c.harness} />
                    </TableCell>
                    <TableCell className="max-w-48 truncate font-mono text-xs" title={c.home}>
                      {c.home}
                    </TableCell>
                    <TableCell className="text-sm" aria-live="polite">
                      {st?.kind === 'pending' && <Loader2 aria-label="Importing" className="size-4 animate-spin" />}
                      {st?.kind === 'ok' && (
                        <span className="inline-flex items-center gap-1 text-green-700 dark:text-green-400">
                          <Check aria-hidden className="size-4" /> Imported
                        </span>
                      )}
                      {st?.kind === 'error' && (
                        <span className="inline-flex items-center gap-1 text-destructive">
                          <X aria-hidden className="size-4" /> {st.message}
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        )}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          <Button type="button" disabled={picked.size === 0 || running} onClick={() => void run()}>
            Import selected{picked.size ? ` (${picked.size})` : ''}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
