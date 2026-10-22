import { memo } from 'react'
import { AXIS_H } from './layout'
import { timeToX, type Viewport } from './viewport'
import { axisTicks, formatAxisLabel, niceStep } from '@/lib/time'

interface Props {
  viewport: Viewport
  width: number
}

/** Time axis with 6 to 12 labels chosen by `niceStep`. */
export const TimeAxis = memo(function TimeAxis({ viewport, width }: Props) {
  const ticks = axisTicks(viewport.t0, viewport.t1, Math.max(3, Math.min(12, Math.floor(width / 70))))
  const step = niceStep(viewport.t1 - viewport.t0, Math.max(3, Math.min(12, Math.floor(width / 70))))
  return (
    <svg width={width} height={AXIS_H} className="block" aria-hidden focusable="false" data-testid="time-axis">
      <line x1={0} x2={width} y1={AXIS_H - 0.5} y2={AXIS_H - 0.5} stroke="var(--border)" />
      {ticks.map((t) => {
        const x = timeToX(t.t, viewport, width)
        return (
          <g key={t.t}>
            <line x1={x} x2={x} y1={t.major ? 12 : 17} y2={AXIS_H} stroke="var(--muted-foreground)" strokeOpacity={0.7} />
            <text x={x + 3} y={11} fontSize={10} fill="var(--muted-foreground)" className="tabular-nums">
              {formatAxisLabel(t.t, step)}
            </text>
          </g>
        )
      })}
    </svg>
  )
})
