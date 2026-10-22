import { useState, type ReactNode } from 'react'
import { useJoinRun, useRun } from '@/api/queries'
import { Duration } from '@/components/common/Duration'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { StateBadge } from '@/components/common/StateBadge'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { formatMoney } from '@/lib/format'
import { BriefTab } from './BriefTab'
import { isTerminal } from './data'
import { EventsTab } from './EventsTab'
import { JoinTab } from './JoinTab'
import { ResultTab } from './ResultTab'
import { RouteTab } from './RouteTab'
import { RunActions } from './RunActions'
import { TranscriptTab } from './TranscriptTab'
import { WorktreeTab } from './WorktreeTab'

const TABS = [
  ['brief', 'Brief'],
  ['route', 'Route'],
  ['events', 'Events'],
  ['transcript', 'Transcript'],
  ['result', 'Result'],
  ['worktree', 'Worktree'],
  ['join', 'Join'],
] as const

interface Props {
  runId: string
  onClose: () => void
}

/**
 * The run panel (design 4.9): a right-hand Sheet, 560 px wide and full width on mobile. It is non-modal so the
 * timeline behind it stays usable: clicking another lane swaps the run. The Sheet stays mounted across runs; the
 * content is keyed on `runId`, so every per-run piece of state (tabs, filters, join result) resets with the run.
 */
export function RunPanel({ runId, onClose }: Props) {
  return (
    <Sheet open modal={false} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="w-full gap-0 sm:max-w-[560px]" onInteractOutside={(e) => e.preventDefault()}>
        <RunPanelBody key={runId} runId={runId} />
      </SheetContent>
    </Sheet>
  )
}

type TabId = (typeof TABS)[number][0]

function RunPanelBody({ runId }: { runId: string }) {
  const run = useRun(runId)
  const r = run.data
  // A tab mounts when first opened and then stays mounted (hidden), so Events and Transcript keep their filter,
  // search and scroll position. The join command is requested by the Join tab once and kept here.
  const [visited, setVisited] = useState<ReadonlySet<TabId>>(new Set<TabId>(['brief']))
  const join = useJoinRun()
  const body = (id: TabId, node: ReactNode) => (
    <TabsContent key={id} value={id} forceMount className="h-full data-[state=inactive]:hidden">
      {visited.has(id) && node}
    </TabsContent>
  )
  return (
    <>
      <SheetHeader className="border-b pr-12">
        <SheetTitle className="truncate">{r?.name ?? runId}</SheetTitle>
        <SheetDescription asChild>
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
            {r ? (
              <>
                <StateBadge state={r.state} />
                {r.harness && <HarnessBadge harness={r.harness} />}
                {r.account && <span>{r.account}</span>}
                {r.model && <span className="text-muted-foreground">{r.model}</span>}
                <Duration start={r.created_at} end={r.finished_at} />
                <span className="tabular-nums">{formatMoney(r.usage.cost)}</span>
              </>
            ) : (
              <span className="text-muted-foreground">{runId}</span>
            )}
          </div>
        </SheetDescription>
      </SheetHeader>

      {run.isPending && (
        <div className="flex flex-col gap-2 p-4" aria-busy="true">
          <Skeleton className="h-8" />
          <Skeleton className="h-40" />
        </div>
      )}
      {run.isError && (
        <div className="p-4">
          <ErrorAlert error={run.error} title="Could not load the run" onRetry={() => void run.refetch()} />
        </div>
      )}
      {r && (
        <>
          <Tabs defaultValue="brief" onValueChange={(v) => setVisited((s) => (s.has(v as TabId) ? s : new Set(s).add(v as TabId)))} className="min-h-0 flex-1 gap-0">
            <TabsList className="mx-4 mt-3 h-auto w-auto flex-wrap justify-start">
              {TABS.map(([id, label]) => (
                <TabsTrigger key={id} value={id}>
                  {label}
                </TabsTrigger>
              ))}
            </TabsList>
            <div className="min-h-0 flex-1 overflow-auto p-4">
              {body('brief', <BriefTab runId={runId} />)}
              {body('route', <RouteTab runId={runId} />)}
              {body('events', <EventsTab runId={runId} />)}
              {body('transcript', <TranscriptTab runId={runId} active={!isTerminal(r.state)} />)}
              {body('result', <ResultTab run={r} />)}
              {body('worktree', <WorktreeTab run={r} />)}
              {body('join', <JoinTab runId={runId} join={join} />)}
            </div>
          </Tabs>
          <RunActions runId={runId} state={r.state} />
        </>
      )}
    </>
  )
}
