import { useState } from 'react'
import { ScrollText } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { RelativeTime } from '@/components/common/RelativeTime'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { useRouteRecords } from '@/api/queries'

export const LOG_PAGE_SIZE = 10

export function RouteLog() {
  const log = useRouteRecords(LOG_PAGE_SIZE)
  const [index, setIndex] = useState(0)
  const pages = log.data?.pages ?? []
  const page = pages[Math.min(index, Math.max(pages.length - 1, 0))]
  const hasNext = index + 1 < pages.length || !!log.hasNextPage

  async function next() {
    if (index + 1 >= pages.length) await log.fetchNextPage()
    setIndex((i) => i + 1)
  }

  if (log.isError && !log.data) return <ErrorAlert error={log.error} title="Could not load the Route log" onRetry={() => void log.refetch()} />
  if (log.isPending) return <TableSkeleton rows={5} columns={6} />
  if (!page || page.items.length === 0) return <EmptyState icon={ScrollText} title="No routes yet" description="Routing decisions appear here as runs are routed." />

  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">When</TableHead>
              <TableHead scope="col">Prompt</TableHead>
              <TableHead scope="col">Choice</TableHead>
              <TableHead scope="col">Account</TableHead>
              <TableHead scope="col">Source</TableHead>
              <TableHead scope="col" className="text-right">
                Confidence
              </TableHead>
              <TableHead scope="col">Run</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {page.items.map((r) => (
              <TableRow key={`${r.ts}-${r.run_id ?? r.prompt_excerpt}`}>
                <TableCell>
                  <RelativeTime value={r.ts} />
                </TableCell>
                <TableCell className="max-w-64 truncate" title={r.prompt_excerpt}>
                  {r.prompt_excerpt}
                </TableCell>
                <TableCell className="font-mono text-xs">{r.result.choice_id}</TableCell>
                <TableCell>{r.result.account}</TableCell>
                <TableCell>
                  <Badge variant="outline">{r.result.source}</Badge>
                </TableCell>
                <TableCell className="text-right font-mono tabular-nums">{r.result.confidence?.toFixed(2) ?? '-'}</TableCell>
                <TableCell className="font-mono text-xs text-muted-foreground">{r.run_id ?? '-'}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <nav aria-label="Route log pages" className="flex items-center justify-end gap-3">
        <span className="text-sm text-muted-foreground" aria-live="polite">
          Page {index + 1}
        </span>
        <Button variant="outline" size="sm" disabled={index === 0} onClick={() => setIndex((i) => i - 1)}>
          Previous
        </Button>
        <Button variant="outline" size="sm" disabled={!hasNext || log.isFetchingNextPage} onClick={() => void next()}>
          Next
        </Button>
      </nav>
    </div>
  )
}
