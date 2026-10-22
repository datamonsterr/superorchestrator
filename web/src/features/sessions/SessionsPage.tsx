import { useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router'
import { Layers } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { PageHeader } from '@/components/layout/PageHeader'
import { CommandBlock } from '@/components/common/CommandBlock'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { useSessions } from '@/api/queries'
import { STATUS_FILTERS, filterFromParams, paramsFromFilter } from './filters'
import { SessionsTable } from './SessionsTable'

const SEARCH_DEBOUNCE_MS = 250

export function SessionsPage() {
  const [params, setParams] = useSearchParams()
  const filter = useMemo(() => filterFromParams(params), [params])
  const [text, setText] = useState(filter.q ?? '')
  // The last q this page wrote to the URL; a different q in the URL came from outside (back/forward, a link).
  const written = useRef(filter.q ?? '')
  const urlQ = filter.q ?? ''
  useEffect(() => {
    if (urlQ !== written.current) {
      written.current = urlQ
      setText(urlQ)
    }
  }, [urlQ])

  // Debounce typing into the URL; the URL is the source of truth for the query.
  useEffect(() => {
    const next = text.trim()
    if (next === urlQ || next === written.current) return
    const id = setTimeout(() => {
      written.current = next
      setParams(paramsFromFilter({ ...filter, q: next || undefined }), { replace: true })
    }, SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(id)
  }, [text, urlQ, filter, setParams])

  const query = useSessions(filter)
  const sessions = query.data?.pages.flatMap((p) => p.items) ?? []
  const filtered = !!(filter.status || filter.q)

  return (
    <>
      <PageHeader title="Sessions" description="Find a session and open its timeline." />
      <div className="mb-3 flex flex-wrap items-center gap-3">
        <ToggleGroup
          type="single"
          variant="outline"
          aria-label="Filter by status"
          value={filter.status ?? 'all'}
          onValueChange={(v) => {
            // Radix emits '' when the active item is pressed again; keep the current choice.
            if (!v) return
            setParams(paramsFromFilter({ ...filter, status: v === 'all' ? undefined : (v as typeof filter.status) }))
          }}
        >
          {STATUS_FILTERS.map((f) => (
            <ToggleGroupItem key={f.value} value={f.value}>
              {f.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <Input
          type="search"
          className="max-w-xs"
          placeholder="Search client, cwd, run"
          aria-label="Search sessions"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
      </div>

      {query.isError && !query.data ? (
        <ErrorAlert error={query.error} title="Could not load sessions" onRetry={() => void query.refetch()} />
      ) : query.isPending ? (
        <TableSkeleton rows={6} columns={7} />
      ) : sessions.length === 0 ? (
        filtered ? (
          <EmptyState
            icon={Layers}
            title="No sessions match"
            description="Try a different status or search."
            action={
              <Button
                variant="outline"
                onClick={() => {
                  setText('')
                  setParams(new URLSearchParams())
                }}
              >
                Clear filters
              </Button>
            }
          />
        ) : (
          <EmptyState
            icon={Layers}
            title="No sessions yet"
            description="Connect an orchestrator to see its sessions here."
            action={<CommandBlock command="superorchestrator install" />}
          />
        )
      ) : (
        <div className="flex flex-col gap-3">
          {query.isError && <ErrorAlert error={query.error} title="Showing stale data" onRetry={() => void query.refetch()} />}
          <div className="overflow-x-auto rounded-md border">
            <SessionsTable sessions={sessions} />
          </div>
          {query.hasNextPage && (
            <div className="flex justify-center">
              <Button variant="outline" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>
                {query.isFetchingNextPage ? 'Loading…' : 'Load more'}
              </Button>
            </div>
          )}
        </div>
      )}
    </>
  )
}
