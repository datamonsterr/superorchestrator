import { lazy, Suspense, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, Maximize, Minus, Plus, Radio } from 'lucide-react'
import { Link, useNavigate, useParams } from 'react-router'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { PageHeader } from '@/components/layout/PageHeader'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Skeleton } from '@/components/ui/skeleton'
import { Slider } from '@/components/ui/slider'
import { Toggle } from '@/components/ui/toggle'
import { GanttCanvas } from './GanttCanvas'
import { Legend, LegendContent } from './Legend'
import { ORCHESTRATOR_LANE } from './model'
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
    <RunPanel key={runId} runId={runId} onClose={onClose} />
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

function TimelineView({ sessionId, renderRunPanel = defaultRenderRunPanel }: TimelinePageProps & { sessionId: string }) {
  const { runId } = useParams()
  const navigate = useNavigate()
  const reduced = usePrefersReducedMotion()
  const { model, session, loadedEvents, loading, error } = useTimelineModel(sessionId)

  // `null` viewport = fit. Following pins the right edge to `now`; any pan or zoom turns it off.
  const [userViewport, setUserViewport] = useState<Viewport | null>(null)
  const [follow, setFollow] = useState(!reduced)
  // Radix puts the thumb's name on the thumb only; shadcn's Slider cannot take one, so name it here.
  const zoomRef = useRef<HTMLDivElement>(null)
  const ready = !!model
  useEffect(() => {
    zoomRef.current?.querySelector('[role="slider"]')?.setAttribute('aria-label', 'Zoom')
  }, [ready])

  const bounds = useMemo(() => (model ? { tMin: model.tMin, tMax: model.tMax } : null), [model])
  const viewport = useMemo<Viewport | null>(() => {
    if (!model || !bounds) return null
    const base = userViewport ?? fitViewport(bounds)
    return follow && model.active && userViewport ? { t0: model.tMax - span(base), t1: model.tMax } : base
  }, [model, bounds, userViewport, follow])

  const change = (v: Viewport) => {
    if (!bounds) return
    setUserViewport(clampViewport(v, bounds))
    setFollow(false)
  }

  const close = () => navigate(`/sessions/${encodeURIComponent(sessionId)}`)
  const select = (id: string) => {
    if (id !== ORCHESTRATOR_LANE) navigate(`/sessions/${encodeURIComponent(sessionId)}/runs/${encodeURIComponent(id)}`)
  }

  useEffect(() => {
    if (!runId) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && navigate(`/sessions/${encodeURIComponent(sessionId)}`)
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [runId, sessionId, navigate])

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
            <div className="flex items-center gap-2 text-xs text-muted-foreground" role="status">
              <span>Loading events ({loadedEvents})</span>
              <Progress value={progress} className="max-w-48" aria-label="Events loaded" />
            </div>
          )}

          {hasRuns || loadedEvents > 0 ? (
            <GanttCanvas model={model} viewport={viewport} onViewportChange={change} selectedRunId={runId} onSelect={select} />
          ) : (
            !loading && <EmptyState title="No runs in this session yet" description="Runs appear here as soon as the orchestrator spawns them." />
          )}
        </>
      )}

      {runId && renderRunPanel(runId, close)}
    </div>
  )
}
