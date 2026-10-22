import { memo, useRef } from 'react'
import { stateFill } from './colors'
import { bucketSegments } from './minimapBuckets'
import type { RunLane, TimelineModel } from './model'
import { panBy, span, timeToX, xToTime, type Viewport } from './viewport'

const H = 36

interface Props {
  model: TimelineModel
  viewport: Viewport
  width: number
  onViewportChange: (v: Viewport) => void
}

const MiniLane = memo(function MiniLane({ lane, y, rowH, tMin, tMax, width }: { lane: RunLane; y: number; rowH: number; tMin: number; tMax: number; width: number }) {
  return (
    <g data-testid="minimap-lane">
      {bucketSegments(lane.segments, tMin, tMax, width).map(({ x, w, segment }, k) => (
        <rect key={k} x={x} y={y} width={w} height={Math.max(rowH - 0.5, 0.8)} fill={stateFill(segment.state)} opacity={0.85} />
      ))}
    </g>
  )
})

/** Lane density strip with a draggable rectangle for the current window. */
export const Minimap = memo(function Minimap({ model, viewport, width, onViewportChange }: Props) {
  const full: Viewport = { t0: model.tMin, t1: model.tMax }
  const bounds = { tMin: model.tMin, tMax: model.tMax }
  const n = Math.max(model.lanes.length, 1)
  const rowH = Math.max(H / n, 1)
  const ref = useRef<SVGSVGElement>(null)
  const drag = useRef<{ grab: number; start: Viewport } | null>(null)

  const rx0 = Math.max(0, timeToX(viewport.t0, full, width))
  const rx1 = Math.min(width, timeToX(viewport.t1, full, width))
  const at = (clientX: number) => xToTime(clientX - ref.current!.getBoundingClientRect().left, full, width)

  function onPointerDown(e: React.PointerEvent<SVGSVGElement>) {
    if (e.button !== 0) return
    const grab = at(e.clientX)
    let start = viewport
    // A click outside the window centres it there; inside, the drag keeps the grab point.
    if (grab < viewport.t0 || grab > viewport.t1) {
      start = panBy(viewport, grab - (viewport.t0 + span(viewport) / 2), bounds)
      onViewportChange(start)
    }
    drag.current = { grab, start }
    e.currentTarget.setPointerCapture?.(e.pointerId)
  }
  function onPointerMove(e: React.PointerEvent<SVGSVGElement>) {
    const d = drag.current
    if (d) onViewportChange(panBy(d.start, at(e.clientX) - d.grab, bounds))
  }
  const end = () => {
    drag.current = null
  }

  return (
    <svg
      ref={ref}
      width={width}
      height={H}
      className="block cursor-grab touch-none rounded-sm bg-muted/40"
      aria-hidden
      focusable="false"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onLostPointerCapture={end}
      data-testid="minimap"
    >
      {model.lanes.map((lane, i) => (
        <MiniLane key={lane.runId} lane={lane} y={i * rowH} rowH={rowH} tMin={model.tMin} tMax={model.tMax} width={width} />
      ))}
      <rect x={rx0} y={0.5} width={Math.max(rx1 - rx0, 3)} height={H - 1} fill="var(--foreground)" fillOpacity={0.08} stroke="var(--foreground)" strokeWidth={1.2} rx={2} data-testid="minimap-window" />
    </svg>
  )
})
