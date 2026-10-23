import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

/** Table-shaped loading placeholder. Announced once, not per cell. */
export function TableSkeleton({ rows = 5, columns = 4 }: { rows?: number; columns?: number }) {
  return (
    <div role="status" aria-busy="true" aria-label="Loading">
      <Table>
        <TableHeader>
          <TableRow>
            {Array.from({ length: columns }, (_, c) => (
              <TableHead key={c}>
                <span className="sr-only">Loading column {c + 1}</span>
                <Skeleton className="h-4 w-20" />
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {Array.from({ length: rows }, (_, r) => (
            <TableRow key={r} data-testid="skeleton-row">
              {Array.from({ length: columns }, (_, c) => (
                <TableCell key={c}>
                  <Skeleton className="h-4 w-full max-w-40" />
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
