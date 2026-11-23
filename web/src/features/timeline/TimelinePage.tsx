import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, Clock, Keyboard, Maximize, Minus, Network, Plus, Radio, Search, X } from 'lucide-react'
import { Link, useNavigate, useParams } from 'react-router'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { PageHeader } from '@/components/layout/PageHeader'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Progress } from '@/components/ui/progress'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Slider } from '@/components/ui/slider'
import { Toggle } from '@/components/ui/toggle'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { AgentGraphView } from './AgentGraphView'
import { GanttCanvas } from './GanttCanvas'
import { PatternDefs } from './glyphs'
import { laneCurrentState, laneTitle } from './layout'
import { Legend, LegendContent } from './Legend'
import { ORCHESTRATOR_LANE } from './model'
import { SummaryTable } from './SummaryTable'
import { useTimelineModel } from './useTimelineModel'
import {
  clampViewport, fitViewport, sliderToSpan, span, spanToSlider, withSpan, zoomAt, type Viewport,
} from './viewport'

export interface TimelinePageProps {
  /** Renders the panel for the selected run (design 4.9). Defaults to the lazily loaded run panel. */
  renderRunPanel?: (runId: string, onClose: () => void) => ReactNode
}

const RunPanel = lazy(() => import('./run-panel/RunPanel').then((m) => ({ default: m.RunPanel })))
const defaultRenderRunPanel = (runId: string, onClose: () => void) => (
  <Suspense fallback={null}>
    <RunPanel runId={runId} onClose={onClose} />
  </Suspense>
)

function usePrefersReducedMotion(): boolean {
  const [reduced] = useState(() => typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches)
  return reduced
}

function TimelineSkeleton() {
  return (
    <div className="flex flex-col gap-1 rounded-md border p-2" aria-busy="true" aria-label="Loading timeline">
      {Array.from({ length: 6 }, (_, i) => (
        <Skeleton key={i} className="h-8" style={{ width: `${95 - i * 9}%` }} />
      ))}
    </div>
  )
}

/** `/sessions/:sessionId` and `/sessions/:sessionId/runs/:runId`. */
export function TimelinePage(props: TimelinePageProps) {
  const { sessionId = '' } = useParams()
  return <TimelineView key={sessionId} sessionId={sessionId} {...props} />
}

function TimelineView({ sessionId, renderRunPanel }: TimelinePageProps & { sessionId: string }) {
  const { runId } = useParams()
  const navigate = useNavigate()
  const reduced = usePrefersReducedMotion()
  const { model, session, loadedEvents, loading, error } = useTimelineModel(sessionId)

  // `null` viewport = fit. Following pins the right edge to `now`; any pan or zoom turns it off.
  const [userViewport, setUserViewport] = useState<Viewport | null>(null)
  const [follow, setFollow] = useState(!reduced)
  const [viewMode, setViewMode] = useState<'timeline' | 'graph'>('timeline')
  const [searchQuery, setSearchQuery] = useState('')
  const [stateFilter, setStateFilter] = useState('all')

  const queryLower = searchQuery.trim().toLowerCase()
  const hasFilter = queryLower !== '' || stateFilter !== 'all'
  const matchingRunIds = useMemo(() => {
    if (!model) return new Set<string>()
    const set = new Set<string>()
    for (const lane of model.lanes) {
      if (lane.runId === ORCHESTRATOR_LANE) {
        if (!queryLower && stateFilter === 'all') set.add(lane.runId)
        else if (queryLower && 'orchestrator coordinator'.includes(queryLower)) set.add(lane.runId)
        continue
      }
      const run = lane.run
      const st = laneCurrentState(lane) ?? run?.state ?? 'queued'
      if (stateFilter !== 'all') {
        if (stateFilter === 'active' && !['starting', 'running', 'waiting'].includes(st)) continue
        if (stateFilter === 'attention' && !['stalled', 'escalated', 'pending_review'].includes(st)) continue
        if (stateFilter === 'succeeded' && st !== 'succeeded') continue
        if (stateFilter === 'failed' && !['failed', 'cancelled'].includes(st)) continue
        if (stateFilter === 'queued' && st !== 'queued') continue
      }
      if (queryLower) {
        const titleMatch = laneTitle(lane).toLowerCase().includes(queryLower)
        const idMatch = lane.runId.toLowerCase().includes(queryLower)
        const harnessMatch = run?.harness?.toLowerCase().includes(queryLower) ?? false
        const acctMatch = run?.account?.toLowerCase().includes(queryLower) ?? false
        const modelMatch = run?.model?.toLowerCase().includes(queryLower) ?? false
        if (!titleMatch && !idMatch && !harnessMatch && !acctMatch && !modelMatch) continue
      }
      set.add(lane.runId)
    }
    return set
  }, [model, queryLower, stateFilter])

  // Radix puts the thumb's name on the thumb only; shadcn's Slider cannot take one, so name it here.
  const zoomRef = useRef<HTMLDivElement>(null)
  const ready = !!model
  useEffect(() => {
    if (viewMode === 'timeline') {
      zoomRef.current?.querySelector('[role="slider"]')?.setAttribute('aria-label', 'Zoom')
    }
  }, [ready, viewMode])

  const tMin = model?.tMin
  const tMax = model?.tMax
  const bounds = useMemo(() => (tMin === undefined || tMax === undefined ? null : { tMin, tMax }), [tMin, tMax])
  let vt0: number | null = null
  let vt1: number | null = null
  if (model && bounds) {
    const base = userViewport ?? fitViewport(bounds)
    const pinned = follow && model.active && userViewport
    vt0 = pinned ? model.tMax - span(base) : base.t0
    vt1 = pinned ? model.tMax : base.t1
  }
  // Stable identity per (t0, t1): memoised children skip renders that do not move the window.
  const viewport = useMemo<Viewport | null>(() => (vt0 === null || vt1 === null ? null : { t0: vt0, t1: vt1 }), [vt0, vt1])

  const boundsRef = useRef(bounds)
  useEffect(() => {
    boundsRef.current = bounds
  })
  const change = useCallback((v: Viewport) => {
    if (!boundsRef.current) return
    setUserViewport(clampViewport(v, boundsRef.current))
    setFollow(false)
  }, [])

  const close = useCallback(() => navigate(`/sessions/${encodeURIComponent(sessionId)}`), [navigate, sessionId])
  const select = useCallback(
    (id: string) => {
      if (id !== ORCHESTRATOR_LANE) navigate(`/sessions/${encodeURIComponent(sessionId)}/runs/${encodeURIComponent(id)}`)
    },
    [navigate, sessionId],
  )

  // The default run panel is a Sheet and closes itself on Escape. A custom `renderRunPanel` may not, so for those
  // Escape closes it, unless it came from inside a nested dialog, list or popper or was already handled.
  const custom = renderRunPanel !== undefined
  useEffect(() => {
    if (!runId || !custom) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return
      const t = e.target instanceof Element ? e.target : null
      if (t?.closest('[role="dialog"],[role="alertdialog"],[role="listbox"],[role="menu"],[data-radix-popper-content-wrapper]')) return
      close()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [runId, custom, close])

  const s = session.data
  const title = s ? `${s.client} ${new Date(s.started_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : 'Session'
  const hasRuns = (s?.runs.length ?? 0) > 0
  const progress = model && s ? Math.min(100, Math.round(((model.lastKey?.ts ?? model.tMin) - model.tMin) / Math.max(1, (s.ended_at ? Date.parse(s.ended_at) : model.tMax) - model.tMin) * 100)) : 0

  return (
    <div className="flex flex-col gap-3">
      <PageHeader
        title="Session timeline"
        description={
          <span className="flex flex-wrap items-center gap-x-2">
            <Link to="/sessions" className="inline-flex items-center gap-1 underline-offset-4 hover:underline">
              <ArrowLeft aria-hidden className="size-3.5" />
              Sessions
            </Link>
            <span aria-hidden>/</span>
            <span>{title}</span>
          </span>
        }
      />

      {session.isError && <ErrorAlert error={session.error} title="Could not load the session" onRetry={() => void session.refetch()} />}
      {error != null && <ErrorAlert error={error} title="Could not load all events" />}
      {session.isPending && <TimelineSkeleton />}

      {model && viewport && bounds && s && (
        <>
          <svg width={0} height={0} className="absolute" aria-hidden focusable="false">
            <PatternDefs />
          </svg>

          {/* View mode toggle, filter bar, and shortcuts */}
          <div className="flex flex-wrap items-center justify-between gap-2.5">
            <div className="flex items-center gap-2 flex-wrap">
              <ToggleGroup
                type="single"
                value={viewMode}
                onValueChange={(v) => v && setViewMode(v as 'timeline' | 'graph')}
                aria-label="View mode"
                variant="outline"
                size="sm"
              >
                <ToggleGroupItem value="timeline" aria-label="Timeline view">
                  <Clock aria-hidden className="size-3.5" />
                  <span>Timeline</span>
                </ToggleGroupItem>
                <ToggleGroupItem value="graph" aria-label="Agent graph view">
                  <Network aria-hidden className="size-3.5" />
                  <span>Agent Graph</span>
                </ToggleGroupItem>
              </ToggleGroup>

              {/* Filter search input */}
              <div className="relative flex items-center min-w-[170px] sm:min-w-[220px]">
                <Search aria-hidden className="absolute left-2.5 size-3.5 text-muted-foreground pointer-events-none" />
                <Input
                  type="search"
                  placeholder="Filter runs…"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="h-8 pl-8 pr-7 text-xs"
                  aria-label="Filter runs"
                />
                {searchQuery && (
                  <button
                    type="button"
                    onClick={() => setSearchQuery('')}
                    className="absolute right-2 text-muted-foreground hover:text-foreground"
                    aria-label="Clear search"
                  >
                    <X aria-hidden className="size-3.5" />
                  </button>
                )}
              </div>

              {/* State filter dropdown */}
              <Select value={stateFilter} onValueChange={setStateFilter}>
                <SelectTrigger className="h-8 w-32 sm:w-36 text-xs" aria-label="Filter by state">
                  <SelectValue placeholder="All states" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All States</SelectItem>
                  <SelectItem value="active">Active</SelectItem>
                  <SelectItem value="attention">Needs Attention</SelectItem>
                  <SelectItem value="succeeded">Succeeded</SelectItem>
                  <SelectItem value="failed">Failed / Cancelled</SelectItem>
                  <SelectItem value="queued">Queued</SelectItem>
                </SelectContent>
              </Select>

              {hasFilter && (
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <span className="tabular-nums font-medium">
                    {matchingRunIds.has(ORCHESTRATOR_LANE) ? matchingRunIds.size - 1 : matchingRunIds.size} of {Math.max(0, model.lanes.length - 1)} runs
                  </span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      setSearchQuery('')
                      setStateFilter('all')
                    }}
                    className="h-7 px-1.5 text-xs text-muted-foreground hover:text-foreground"
                  >
                    Reset
                  </Button>
                </div>
              )}
            </div>

            {/* Shortcuts help button */}
            <Popover>
              <PopoverTrigger asChild>
                <Button type="button" variant="outline" size="sm" aria-label="Keyboard shortcuts" title="Keyboard shortcuts" className="gap-1.5">
                  <Keyboard aria-hidden className="size-3.5" />
                  <span className="hidden sm:inline">Shortcuts</span>
                </Button>
              </PopoverTrigger>
              <PopoverContent align="end" className="w-80 p-3 text-xs">
                <div className="font-semibold mb-2 text-foreground">Keyboard Shortcuts</div>
                <div className="space-y-2">
                  <div>
                    <div className="font-medium text-muted-foreground mb-1">Timeline (Gantt)</div>
                    <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1">
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">+</kbd><span>Zoom in</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">-</kbd><span>Zoom out</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">0</kbd><span>Fit whole session</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">← / →</kbd><span>Pan plot / walk segments</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">↑ / ↓</kbd><span>Move between runs</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">Enter</kbd><span>Open run details</span>
                    </div>
                  </div>
                  <div className="border-t pt-1.5">
                    <div className="font-medium text-muted-foreground mb-1">Agent Graph</div>
                    <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1">
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">Arrows</kbd><span>Navigate nodes (parent/child/sibling)</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">Enter / Space</kbd><span>Open run details</span>
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">Home / End</kbd><span>Jump to Root / Last node</span>
                    </div>
                  </div>
                  <div className="border-t pt-1.5">
                    <div className="grid grid-cols-[auto_1fr] gap-x-2">
                      <kbd className="rounded border bg-muted px-1 font-mono text-[10px]">Esc</kbd><span>Close details panel</span>
                    </div>
                  </div>
                </div>
              </PopoverContent>
            </Popover>
          </div>

          {viewMode === 'timeline' ? (
            <>
              <div className="flex flex-wrap items-center gap-2" role="toolbar" aria-label="Timeline controls">
                <Button type="button" variant="outline" size="sm" onClick={() => { setUserViewport(null); setFollow(false) }}>
                  <Maximize aria-hidden />
                  Fit
                </Button>
                <Button type="button" variant="outline" size="icon-sm" aria-label="Zoom out" onClick={() => change(zoomAt(viewport, 1 / 0.6, viewport.t0 + span(viewport) / 2, bounds))}>
                  <Minus aria-hidden />
                </Button>
                <div ref={zoomRef} role="group" aria-label="Zoom" className="flex w-32 items-center sm:w-48">
                <Slider
                  className="w-full"
                  min={0}
                  max={100}
                  step={1}
                  value={[spanToSlider(span(viewport), bounds)]}
                  onValueChange={([v]) => change(withSpan(viewport, sliderToSpan(v, bounds), bounds))}
                />
                </div>
                <Button type="button" variant="outline" size="icon-sm" aria-label="Zoom in" onClick={() => change(zoomAt(viewport, 0.6, viewport.t0 + span(viewport) / 2, bounds))}>
                  <Plus aria-hidden />
                </Button>
                {reduced ? (
                  <Button type="button" variant="outline" size="sm" disabled={!model.active} onClick={() => { setUserViewport(userViewport ?? viewport); setFollow(true) }}>
                    <Radio aria-hidden />
                    Jump to live
                  </Button>
                ) : (
                  <Toggle
                    variant="outline"
                    size="sm"
                    pressed={follow && model.active}
                    disabled={!model.active}
                    onPressedChange={(on) => {
                      setUserViewport(userViewport ?? viewport)
                      setFollow(on)
                    }}
                    aria-label="Follow live"
                  >
                    <Radio aria-hidden />
                    Live
                  </Toggle>
                )}
                <div className="ml-auto flex items-center gap-2">
                  <Legend />
                </div>
              </div>
              <div className="hidden xl:block">
                <LegendContent compact />
              </div>

              {loading && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-busy="true">
                  <span>Loading events…</span>
                  <Progress value={progress} className="max-w-48" aria-label="Events loaded" />
                </div>
              )}

              {hasRuns || loadedEvents > 0 ? (
                <GanttCanvas
                  model={model}
                  viewport={viewport}
                  onViewportChange={change}
                  selectedRunId={runId}
                  onSelect={select}
                  matchingRunIds={matchingRunIds}
                  hasFilter={hasFilter}
                />
              ) : (
                !loading && <EmptyState title="No runs in this session yet" description="Runs appear here as soon as the orchestrator spawns them." />
              )}
            </>
          ) : (
            <>
              {loading && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground" role="status" aria-busy="true">
                  <span>Loading events…</span>
                  <Progress value={progress} className="max-w-48" aria-label="Events loaded" />
                </div>
              )}

              {hasRuns || loadedEvents > 0 ? (
                <>
                  <AgentGraphView
                    model={model}
                    selectedRunId={runId}
                    onSelect={select}
                    searchQuery={searchQuery}
                    stateFilter={stateFilter}
                  />
                  <SummaryTable model={model} />
                </>
              ) : (
                !loading && <EmptyState title="No runs in this session yet" description="Runs appear here as soon as the orchestrator spawns them." />
              )}
            </>
          )}
        </>
      )}

      {runId && (renderRunPanel ?? defaultRenderRunPanel)(runId, close)}
    </div>
  )
}
