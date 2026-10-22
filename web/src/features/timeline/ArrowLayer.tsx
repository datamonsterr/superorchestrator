import { memo } from 'react'
import { LANE_H, MAX_ARROWS } from './layout'
import type { Arrow } from './model'
import { timeToX, visibleRange, type Viewport } from './viewport'

interface Props {
  arrows: readonly Arrow[]
  /** Top y of each rendered lane. Arrows with an unrendered endpoint are skipped. */
  laneY: ReadonlyMap<string, number>
  viewport: Viewport
  width: number
  onHover: (a: Arrow | null) => void
}

/** Curved message arrows between lanes, capped at 200 with a "+N more" chip (design 4.5). */
export const ArrowLayer = memo(function ArrowLayer({ arrows, laneY, viewport, width, onHover }: Props) {
  const [from, to] = visibleRange(arrows, viewport.t0, viewport.t1, (a) => a.at)
  const drawn: { a: Arrow; i: number; x: number; y1: number; y2: number }[] = []
  let hidden = 0
  for (let i = from; i < to; i++) {
    const a = arrows[i]
    if (a.at < viewport.t0 || a.at > viewport.t1) continue
    const ya = laneY.get(a.from)
    const yb = laneY.get(a.to)
    if (ya === undefined || yb === undefined || ya === yb) continue
    if (drawn.length >= MAX_ARROWS) {
      hidden++
      continue
    }
    const down = yb > ya
    drawn.push({ a, i, x: timeToX(a.at, viewport, width), y1: ya + LANE_H / 2 + (down ? 8 : -8), y2: yb + LANE_H / 2 + (down ? -9 : 9) })
  }
  return (
    <g>
      {drawn.map(({ a, i, x, y1, y2 }) => {
        const d = `M${x} ${y1} C${x + 18} ${y1}, ${x + 18} ${y2}, ${x} ${y2}`
        return (
          <g key={i} onPointerEnter={() => onHover(a)} onPointerLeave={() => onHover(null)}>
            <path d={d} fill="none" stroke="var(--muted-foreground)" strokeWidth={1.3} markerEnd="url(#so-arrow-head)" pointerEvents="none" />
            <path d={d} fill="none" stroke="transparent" strokeWidth={9} data-tip="arrow" data-i={i} className="cursor-pointer" />
          </g>
        )
      })}
      {hidden > 0 && (
        <g transform={`translate(${width - 8} 4)`} pointerEvents="none">
          <rect x={-74} y={0} width={74} height={16} rx={8} fill="var(--popover)" stroke="var(--border)" />
          <text x={-37} y={11.5} textAnchor="middle" fontSize={10} fill="var(--popover-foreground)">
            +{hidden} more
          </text>
        </g>
      )}
    </g>
  )
})
