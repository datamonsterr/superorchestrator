import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowLayer } from './ArrowLayer'
import { PatternDefs } from './glyphs'
import { LaneLabel } from './LaneLabel'
import { LaneRow } from './LaneRow'
import { Minimap } from './Minimap'
import { PlotTooltip } from './PlotTooltip'
import { SummaryTable } from './SummaryTable'
import { TimeAxis } from './TimeAxis'
import { AXIS_H, describeSegment, LANE_H, VIRTUALISE_ABOVE } from './layout'
import { type Arrow, type TimelineModel } from './model'
import { fitViewport, panBy, span, timeToX, xToTime, zoomAt, type Viewport } from './viewport'
import { STATE_CSS } from './colors'
import { axisTicks } from '@/lib/time'
import { cn } from '@/lib/utils'

export interface GanttCanvasProps {
  model: TimelineModel
  viewport: Viewport
  onViewportChange: (v: Viewport) => void
  selectedRunId?: string | null
  /** Called with a lane id when a lane label, bar or marker is clicked, or Enter is pressed on a lane. */
  onSelect: (runId: string) => void
  className?: string
}

const FALLBACK_WIDTH = 960

/** Tracks an element's content width through ResizeObserver (callback ref, no effect state). */
function useWidth(): [(el: HTMLElement | null) => void, number] {
  const [width, setWidth] = useState(FALLBACK_WIDTH)
  const roRef = useRef<ResizeObserver | null>(null)
  const ref = useCallback((el: HTMLElement | null) => {
    roRef.current?.disconnect()
    roRef.current = null
    if (!el) return
    const measure = (w: number) => w > 0 && setWidth((prev) => (Math.abs(prev - w) < 0.5 ? prev : Math.floor(w)))
    measure(el.getBoundingClientRect().width)
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver((entries) => measure(entries[0].contentRect.width))
    ro.observe(el)
    roRef.current = ro
  }, [])
  return [ref, width]
}

/**
 * The Gantt plot (design 4): SVG, culled to the viewport, lanes virtualised above 60. It renders only from
 * `TimelineModel` and a `Viewport`, so a canvas renderer could replace it without touching derivation.
 */
export function GanttCanvas({ model, viewport, onViewportChange, selectedRunId, onSelect, className }: GanttCanvasProps) {
  const [plotRef, width] = useWidth()
  const plotElRef = useRef<HTMLDivElement | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const setPlot = useCallback(
    (el: HTMLDivElement | null) => {
      plotElRef.current = el
      plotRef(el)
    },
    [plotRef],
  )
  const bounds = useMemo(() => ({ tMin: model.tMin, tMax: model.tMax }), [model.tMin, model.tMax])
  const [hoverArrow, setHoverArrow] = useState<Arrow | null>(null)
  const [active, setActive] = useState<{ lane: string; i: number } | null>(null)
  const [announce, setAnnounce] = useState('')
  const [tabLane, setTabLane] = useState<string | null>(null)
  const helpId = useId()

  const lanes = model.lanes
  const virtualise = lanes.length > VIRTUALISE_ABOVE
  // eslint-disable-next-line react-hooks/incompatible-library -- virtualizer results are used directly, never memoised
  const virtualizer = useVirtualizer({
    count: lanes.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => LANE_H,
    overscan: 8,
    enabled: virtualise,
  })
  const rendered: number[] = virtualise ? virtualizer.getVirtualItems().map((v) => v.index) : lanes.map((_, i) => i)
  const totalH = lanes.length * LANE_H

  // Latest values for the native (non-passive) wheel listener and window drag handlers.
  const latest = useRef({ viewport, width, bounds, onViewportChange })
  useEffect(() => {
    latest.current = { viewport, width, bounds, onViewportChange }
  })

  useEffect(() => {
    const el = plotElRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      const { viewport: vp, width: w, bounds: b, onViewportChange: change } = latest.current
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault()
        const rect = el.getBoundingClientRect()
        change(zoomAt(vp, Math.exp(e.deltaY * 0.0015), xToTime(e.clientX - rect.left, vp, w), b))
      } else if (e.shiftKey) {
        e.preventDefault()
        change(panBy(vp, ((e.deltaY || e.deltaX) / w) * span(vp), b))
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [])

  // Drag to pan. Pointer capture starts once the pointer has moved past a small threshold, so a plain click still
  // reaches the bar under it. A drag suppresses the click that ends it.
  const suppressClick = useRef(false)
  const suppressTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const drag = useRef<{ x0: number; vp0: Viewport; moved: boolean } | null>(null)
  useEffect(() => () => clearTimeout(suppressTimer.current), [])
  function onPointerDown(e: React.PointerEvent) {
    if (e.button !== 0) return
    drag.current = { x0: e.clientX, vp0: viewport, moved: false }
  }
  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    const d = drag.current
    if (!d) return
    if (e.buttons === 0) return endDrag() // the button was released outside the window
    const dx = e.clientX - d.x0
    if (!d.moved) {
      if (Math.abs(dx) < 4) return
      d.moved = true
      e.currentTarget.setPointerCapture?.(e.pointerId)
    }
    onViewportChange(panBy(d.vp0, -(dx / width) * span(d.vp0), bounds))
  }
  const endDrag = () => {
    const d = drag.current
    drag.current = null
    if (d?.moved) {
      suppressClick.current = true
      clearTimeout(suppressTimer.current)
      suppressTimer.current = setTimeout(() => (suppressClick.current = false), 0)
    }
  }
  function onPlotClick(e: React.MouseEvent) {
    if (suppressClick.current) return
    const laneId = (e.target as Element).closest?.('[data-lane]')?.getAttribute('data-lane')
    if (laneId) onSelect(laneId)
  }

  const centre = viewport.t0 + span(viewport) / 2
  function onRootKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const tag = (e.target as HTMLElement).tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || e.altKey || e.ctrlKey || e.metaKey) return
    switch (e.key) {
      case '+':
      case '=':
        e.preventDefault()
        return onViewportChange(zoomAt(viewport, 0.6, centre, bounds))
      case '-':
      case '_':
        e.preventDefault()
        return onViewportChange(zoomAt(viewport, 1 / 0.6, centre, bounds))
      case '0':
        e.preventDefault()
        return onViewportChange(fitViewport(bounds))
      case 'ArrowLeft':
      case 'ArrowRight': {
        if (e.target !== e.currentTarget) return
        e.preventDefault()
        const step = span(viewport) * (e.shiftKey ? 0.5 : 0.1)
        return onViewportChange(panBy(viewport, e.key === 'ArrowLeft' ? -step : step, bounds))
      }
    }
  }

  // Lane keyboard model: Enter opens, Left/Right walk the lane's segments, Up/Down move between lanes.
  function onLaneKeyDown(e: KeyboardEvent<HTMLUListElement>) {
    const li = (e.target as HTMLElement).closest('li[data-lane]') as HTMLElement | null
    if (!li || e.target !== li) return
    const laneId = li.dataset.lane!
    const lane = lanes[model.index[laneId]]
    if (!lane) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      return onSelect(laneId)
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const sib = (e.key === 'ArrowDown' ? li.nextElementSibling : li.previousElementSibling) as HTMLElement | null
      return sib?.focus()
    }
    if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && lane.segments.length > 0) {
      e.preventDefault()
      e.stopPropagation()
      const n = lane.segments.length
      const cur = active?.lane === laneId ? active.i : null
      const i = cur === null ? (e.key === 'ArrowRight' ? 0 : n - 1) : Math.min(Math.max(cur + (e.key === 'ArrowRight' ? 1 : -1), 0), n - 1)
      const seg = lane.segments[i]
      setActive({ lane: laneId, i })
      setAnnounce(describeSegment(lane, seg))
      if (seg.end < viewport.t0 || seg.start > viewport.t1) onViewportChange(panBy(viewport, (seg.start + seg.end) / 2 - centre, bounds))
    }
  }

  // Memoised on the rendered range, so the arrow layer only redraws when lanes or the visible window change.
  const renderedKey = rendered.join(',')
  const laneY = useMemo(() => {
    const m = new Map<string, number>()
    if (renderedKey) for (const k of renderedKey.split(',')) m.set(lanes[Number(k)].runId, Number(k) * LANE_H)
    return m
  }, [lanes, renderedKey])

  const stopId = rendered.some((i) => lanes[i].runId === tabLane)
    ? tabLane
    : rendered.some((i) => lanes[i].runId === selectedRunId)
      ? selectedRunId
      : lanes[rendered[0]]?.runId

  const highlighted = useMemo(() => new Set(hoverArrow ? [hoverArrow.from, hoverArrow.to] : []), [hoverArrow])
  const ticks = useMemo(() => axisTicks(viewport.t0, viewport.t1, Math.max(3, Math.min(12, Math.floor(width / 70)))), [viewport.t0, viewport.t1, width])
  const nowX = model.active ? timeToX(model.now, viewport, width) : null

  return (
    <>
    <div
      role="group"
      aria-label="Session timeline"
      aria-describedby={helpId}
      tabIndex={0}
      onKeyDown={onRootKeyDown}
      className={cn('flex min-w-0 flex-col overflow-hidden rounded-md border bg-background outline-none focus-visible:ring-2 focus-visible:ring-ring', className)}
      data-testid="gantt"
    >
      <style href="so-gantt-state-colors" precedence="default">
        {STATE_CSS}
      </style>
      <p id={helpId} className="sr-only">
        Plus and minus zoom, zero fits the whole session, arrow keys pan, tab moves to the run list. In the run list, up
        and down move between runs, left and right move between segments, and Enter opens the run.
      </p>
      <svg width={0} height={0} className="absolute" aria-hidden focusable="false">
        <PatternDefs />
      </svg>

      <div className="flex border-b">
        <div className="flex w-12 shrink-0 items-end px-2 pb-1 text-[10px] font-medium text-muted-foreground uppercase sm:w-56">
          <span className="hidden sm:inline">Runs</span>
        </div>
        <div className="min-w-0 flex-1 overflow-hidden" style={{ height: AXIS_H }}>
          <TimeAxis viewport={viewport} width={width} />
        </div>
      </div>

      <div ref={scrollRef} className="relative max-h-[min(65vh,720px)] overflow-x-hidden overflow-y-auto" data-testid="gantt-scroll">
        <div className="flex" style={{ height: totalH }}>
          <ul
            role="list"
            aria-label="Runs"
            className="relative w-12 shrink-0 border-r sm:w-56"
            onKeyDown={onLaneKeyDown}
            onFocus={(e) => {
              const id = (e.target as HTMLElement).closest('li[data-lane]')?.getAttribute('data-lane')
              if (!id) return
              setTabLane(id)
              if (active && active.lane !== id) setActive(null)
            }}
          >
            {rendered.map((i) => (
              <LaneLabel
                key={lanes[i].runId}
                lane={lanes[i]}
                y={i * LANE_H}
                selected={lanes[i].runId === selectedRunId}
                onSelect={onSelect}
                tabStop={lanes[i].runId === stopId}
                position={i + 1}
                setSize={lanes.length}
              />
            ))}
          </ul>

          <div
            ref={setPlot}
            className="relative min-w-0 flex-1 touch-pan-y select-none"
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={endDrag}
            onPointerCancel={endDrag}
            onLostPointerCapture={endDrag}
            onClick={onPlotClick} data-testid="gantt-plot">
            <svg width={width} height={totalH} className="block" role="presentation" focusable="false">
              {ticks.map((t) => (
                <line key={t.t} x1={timeToX(t.t, viewport, width)} x2={timeToX(t.t, viewport, width)} y1={0} y2={totalH} stroke="var(--border)" strokeOpacity={t.major ? 1 : 0.5} />
              ))}
              {rendered.map((i) => {
                const lane = lanes[i]
                return (
                  <LaneRow
                    key={lane.runId}
                    lane={lane}
                    y={i * LANE_H}
                    t0={viewport.t0}
                    t1={viewport.t1}
                    width={width}
                    selected={lane.runId === selectedRunId}
                    highlighted={highlighted.has(lane.runId)}
                    activeSegment={active?.lane === lane.runId ? active.i : null}
                  />
                )
              })}
              <ArrowLayer arrows={model.arrows} laneY={laneY} viewport={viewport} width={width} onHover={setHoverArrow} />
              {nowX !== null && nowX >= 0 && nowX <= width && (
                <line x1={nowX} x2={nowX} y1={0} y2={totalH} stroke="var(--ring)" strokeWidth={1.5} strokeDasharray="4 3" pointerEvents="none" data-testid="now-line" />
              )}
            </svg>
            <PlotTooltip containerRef={plotElRef} model={model} />
          </div>
        </div>
      </div>

      <div className="flex border-t">
        <div className="hidden w-56 shrink-0 items-center px-2 text-[10px] font-medium text-muted-foreground uppercase sm:flex">Overview</div>
        <div className="min-w-0 flex-1 p-1">
          <Minimap model={model} viewport={viewport} width={Math.max(width - 8, 10)} onViewportChange={onViewportChange} />
        </div>
      </div>

    </div>
      <div aria-live="polite" role="status" className="sr-only" data-testid="gantt-announce">
        {announce}
      </div>
      <SummaryTable model={model} />
    </>
  )
}
