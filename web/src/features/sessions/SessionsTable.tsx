import { Link } from 'react-router'
import { ChevronRight } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { RelativeTime } from '@/components/common/RelativeTime'
import { formatMoney } from '@/lib/format'
import type { SessionStatus, SessionSummary } from '@/api/types'

const STATUS: Record<SessionStatus, { label: string; dot: string }> = {
  active: { label: 'Active', dot: 'bg-green-600 dark:bg-green-500' },
  escalated: { label: 'Escalated', dot: 'bg-red-600 dark:bg-red-400' },
  failed: { label: 'Failed', dot: 'bg-red-800 dark:bg-red-500' },
  done: { label: 'Done', dot: 'bg-zinc-400' },
}

export function SessionStatusBadge({ status }: { status: SessionStatus }) {
  const s = STATUS[status]
  return (
    <Badge variant="outline" className="gap-1.5" data-status={status}>
      <span aria-hidden className={`size-2 rounded-full ${s.dot}`} />
      {s.label}
    </Badge>
  )
}

export function SessionsTable({ sessions }: { sessions: SessionSummary[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">Started</TableHead>
          <TableHead scope="col">Kind</TableHead>
          <TableHead scope="col">Client</TableHead>
          <TableHead scope="col">Working directory</TableHead>
          <TableHead scope="col" className="text-right">
            Runs
          </TableHead>
          <TableHead scope="col">Status</TableHead>
          <TableHead scope="col" className="text-right">
            Cost
          </TableHead>
          <TableHead scope="col">
            <span className="sr-only">Open</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sessions.map((s) => (
          <TableRow key={s.id} data-session={s.id}>
            <TableCell>
              <RelativeTime value={s.started_at} />
            </TableCell>
            <TableCell className="text-muted-foreground">{s.kind}</TableCell>
            <TableCell className="font-medium">{s.client}</TableCell>
            <TableCell className="max-w-56 truncate font-mono text-xs text-muted-foreground" title={s.cwd}>
              {s.cwd}
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">{s.run_count}</TableCell>
            <TableCell>
              <SessionStatusBadge status={s.status} />
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">{formatMoney(s.cost)}</TableCell>
            <TableCell>
              <Link
                to={`/sessions/${s.id}`}
                aria-label={`Open session ${s.client} ${s.id}`}
                className="inline-flex items-center rounded-md p-1 text-muted-foreground hover:text-foreground"
              >
                <ChevronRight aria-hidden className="size-4" />
              </Link>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
