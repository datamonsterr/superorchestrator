import { memo, useRef } from 'react'
import { stateFill } from './colors'
import type { TimelineModel } from './model'
import { panBy, span, timeToX, xToTime, type Viewport } from './viewport'

const H = 36

interface Props {
  model: TimelineModel
  viewport: Viewport
  width: number
  onViewportChange: (v: Viewport) => void
}

/** Lane density strip with a draggable rectangle for the current window. */
export const Minimap = memo(function Minimap({ model, viewport, width, onViewportChange }: Props) {
  const full: Viewport = { t0: model.tMin, t1: model.tMax }
  const bounds = { tMin: model.tMin, tMax: model.tMax }
  const n = Math.max(model.lanes.length, 1)
  const rowH = Math.max(H / n, 1)
  const ref = useRef<SVGSVGElement>(null)

  const rx0 = Math.max(0, timeToX(viewport.t0, full, width))
  const rx1 = Math.min(width, timeToX(viewport.t1, full, width))

  function onPointerDown(e: React.PointerEvent<SVGSVGElement>) {
    if (e.button !== 0) return
    const box = ref.current!.getBoundingClientRect()
    const at = (clientX: number) => xToTime(clientX - box.left, full, width)
    const grab = at(e.clientX)
    let vp = viewport
    // A click outside the window centres it there; inside, the drag keeps the grab point.
    if (grab < viewport.t0 || grab > viewport.t1) {
      vp = panBy(viewport, grab - (viewport.t0 + span(viewport) / 2), bounds)
      onViewportChange(vp)
    }
    const start = vp
    const move = (ev: PointerEvent) => onViewportChange(panBy(start, at(ev.clientX) - grab, bounds))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <svg ref={ref} width={width} height={H} className="block cursor-grab touch-none rounded-sm bg-muted/40" aria-hidden focusable="false" onPointerDown={onPointerDown} data-testid="minimap">
      {model.lanes.map((lane, i) =>
        lane.segments.map((s, k) => {
          const x0 = timeToX(s.start, full, width)
          const w = timeToX(s.end, full, width) - x0
          if (w < 0.4) return null
          return <rect key={`${i}-${k}`} x={x0} y={i * rowH} width={w} height={Math.max(rowH - 0.5, 0.8)} fill={stateFill(s.state)} opacity={0.85} />
        }),
      )}
      <rect x={rx0} y={0.5} width={Math.max(rx1 - rx0, 3)} height={H - 1} fill="var(--foreground)" fillOpacity={0.08} stroke="var(--foreground)" strokeWidth={1.2} rx={2} data-testid="minimap-window" />
    </svg>
  )
})
