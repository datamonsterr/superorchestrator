import { useEffect, useMemo, useRef, useState } from 'react'
import { ChevronRight } from 'lucide-react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { TranscriptLine } from '@/api/types'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { useTranscriptLines } from './hooks'
import { useTabActive } from './tabActive'

const ROLE_STYLE: Record<TranscriptLine['role'], string> = {
  user: 'border-l-primary bg-primary/5',
  assistant: 'border-l-foreground/30',
  tool: 'border-l-muted-foreground bg-muted/40',
  system: 'border-l-border bg-muted/20 text-muted-foreground',
}

function Line({ line }: { line: TranscriptLine }) {
  const base = cn('border-l-2 px-3 py-1.5 text-sm', ROLE_STYLE[line.role])
  if (line.role === 'tool') {
    return (
      <Collapsible className={base}>
        <CollapsibleTrigger className="group flex w-full items-center gap-1 text-left font-mono text-xs">
          <ChevronRight aria-hidden className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
          <span>tool: {line.tool?.name ?? 'call'}</span>
          {line.tool?.ok !== undefined && <span className={line.tool.ok ? 'text-muted-foreground' : 'text-destructive'}>{line.tool.ok ? 'ok' : 'failed'}</span>}
        </CollapsibleTrigger>
        <CollapsibleContent>
          <pre className="mt-1 overflow-x-auto font-mono text-xs whitespace-pre-wrap">{line.text}</pre>
        </CollapsibleContent>
      </Collapsible>
    )
  }
  return (
    <div className={base}>
      <span className="mr-2 text-[10px] font-semibold text-muted-foreground uppercase">{line.role}</span>
      <span className="break-words whitespace-pre-wrap">{line.text}</span>
    </div>
  )
}

/** Virtualised, variable-height transcript with client-side search and bottom-follow while the run is active. */
export function TranscriptTab({ runId, active }: { runId: string; active: boolean }) {
  const tabActive = useTabActive()
  const t = useTranscriptLines(runId, active && tabActive)
  const [query, setQuery] = useState('')
  const scrollRef = useRef<HTMLDivElement>(null)
  const atBottom = useRef(true)

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? t.lines.filter((l) => l.text.toLowerCase().includes(q)) : t.lines
  }, [t.lines, query])
  // eslint-disable-next-line react-hooks/incompatible-library -- results are used directly
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scrollRef.current, estimateSize: () => 36, overscan: 10 })

  const last = rows.length - 1
  // Follow the bottom only while this tab is shown; on becoming visible again, restore the bottom if it was there.
  useEffect(() => {
    if (tabActive && active && atBottom.current && last >= 0) virtualizer.scrollToIndex(last, { align: 'end' })
  }, [tabActive, active, last, virtualizer])

  if (t.loading && t.lines.length === 0) return <Skeleton className="h-40" />
  if (t.error && t.lines.length === 0) return <ErrorAlert error={t.error} title="Could not load the transcript" onRetry={t.retry} />

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <Input type="search" placeholder="Search loaded lines" aria-label="Search transcript" value={query} onChange={(e) => setQuery(e.target.value)} />
      <p className="text-xs text-muted-foreground">
        {query ? `${rows.length} of ${t.lines.length} lines match` : `${t.lines.length} lines`}
        {t.next !== null && !t.error ? ' (loading more)' : ''}
      </p>
      {t.error != null && <ErrorAlert error={t.error} title="Could not load more lines" onRetry={t.retry} />}
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 overflow-auto rounded-md border"
        role="region"
        aria-label="Transcript"
        tabIndex={0}
        onScroll={(e) => {
          const el = e.currentTarget
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40
        }}
      >
        {rows.length === 0 ? (
          <p className="p-3 text-sm text-muted-foreground">{query ? 'No lines match.' : 'The transcript is empty.'}</p>
        ) : (
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {virtualizer.getVirtualItems().map((v) => (
              <div key={rows[v.index].offset} data-index={v.index} ref={virtualizer.measureElement} className="absolute inset-x-0" style={{ top: 0, transform: `translateY(${v.start}px)` }}>
                <Line line={rows[v.index]} />
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
