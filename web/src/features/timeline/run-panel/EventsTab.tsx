import { useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { Event } from '@/api/types'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { stringifyJson } from '@/lib/format'
import { formatClock } from '@/lib/time'
import { cn } from '@/lib/utils'
import { EVENT_GROUPS, groupOf, summarize, type EventGroup } from './data'
import { useAllRunEvents } from './hooks'

const ALL = EVENT_GROUPS.map((g) => g.id)

function EventRow({ e, open, onToggle }: { e: Event; open: boolean; onToggle: () => void }) {
  return (
    <div className="border-b text-sm">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none">
        <time className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums" dateTime={e.ts}>
          {formatClock(Date.parse(e.ts), true)}
        </time>
        <Badge variant="outline" className="shrink-0 font-mono text-[10px]">
          {e.type}
        </Badge>
        <span className="min-w-0 flex-1 truncate">{summarize(e)}</span>
      </button>
      {open && <pre className="mx-2 mb-2 max-h-64 overflow-auto rounded-md border bg-muted/40 p-2 font-mono text-xs">{stringifyJson(e.data)}</pre>}
    </div>
  )
}

/** Virtualised event list (10 000 rows), filtered by type group; a click expands the raw data. */
export function EventsTab({ runId }: { runId: string }) {
  const events = useAllRunEvents(runId)
  const [groups, setGroups] = useState<EventGroup[]>(ALL)
  const [openId, setOpenId] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const rows = useMemo(() => (events.data ?? []).filter((e) => groups.includes(groupOf(e.type))), [events.data, groups])
  // eslint-disable-next-line react-hooks/incompatible-library -- results are used directly
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scrollRef.current, estimateSize: () => 34, overscan: 12 })

  if (events.isPending) return <Skeleton className="h-40" />
  if (events.isError) return <ErrorAlert error={events.error} onRetry={() => void events.refetch()} />

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <ToggleGroup type="multiple" variant="outline" size="sm" value={groups} onValueChange={(v) => setGroups(v as EventGroup[])} aria-label="Event groups" className="flex-wrap">
        {EVENT_GROUPS.map((g) => (
          <ToggleGroupItem key={g.id} value={g.id}>
            {g.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <p className="text-xs text-muted-foreground" role="status">
        {rows.length} of {events.data.length} events
      </p>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto rounded-md border" role="list" aria-label="Events" tabIndex={0}>
        {rows.length === 0 ? (
          <p className="p-3 text-sm text-muted-foreground">No events in the selected groups.</p>
        ) : (
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {virtualizer.getVirtualItems().map((v) => {
              const e = rows[v.index]
              return (
                <div key={e.id} role="listitem" data-index={v.index} ref={virtualizer.measureElement} className={cn('absolute inset-x-0')} style={{ top: 0, transform: `translateY(${v.start}px)` }}>
                  <EventRow e={e} open={openId === e.id} onToggle={() => setOpenId(openId === e.id ? null : e.id)} />
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
