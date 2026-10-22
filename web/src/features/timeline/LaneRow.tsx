import { memo } from 'react'
import { stateFill, stateInkVar, STATE_STYLES } from './colors'
import { EndGlyph, MarkerGlyph, StateFill } from './glyphs'
import { ATTEMPT_H, ATTEMPT_Y, BAR_H, BAR_Y, LANE_H, TICK_Y1, TICK_Y2, accountAt } from './layout'
import type { RunLane } from './model'
import { clusterTicks, timeToX, visibleRange } from './viewport'

interface Props {
  lane: RunLane
  y: number
  t0: number
  t1: number
  width: number
  selected: boolean
  highlighted: boolean
  /** Index of the keyboard-focused segment in this lane, if any. */
  activeSegment: number | null
}

const ATTEMPT_FILL = {
  running: 'var(--muted-foreground)',
  ok: 'var(--state-succeeded)',
  failover: 'var(--state-stalled)',
  failed: 'var(--state-failed)',
} as const

const clampX = (x: number, width: number) => Math.min(Math.max(x, -40), width + 40)

/**
 * One lane of the plot, culled to the viewport: segments, attempts and spans by binary search, ticks clustered in
 * 3 px buckets, markers. Memoised on lane identity and viewport, so a live append re-renders only the touched lane.
 */
export const LaneRow = memo(function LaneRow({ lane, y, t0, t1, width, selected, highlighted, activeSegment }: Props) {
  const vp = { t0, t1 }
  const padT = (12 / width) * (t1 - t0)
  const x = (t: number) => timeToX(t, vp, width)
  const id = lane.runId

  const [sFrom, sTo] = visibleRange(lane.segments, t0 - padT, t1 + padT, (s) => s.start)
  const [aFrom, aTo] = visibleRange(lane.attempts, t0 - padT, t1 + padT, (s) => s.start)
  const [mFrom, mTo] = visibleRange(lane.markers, t0 - padT, t1 + padT, (s) => s.at)
  const clusters = clusterTicks(lane.ticks, vp, width)

  return (
    <g transform={`translate(0 ${y})`} data-lane={id} data-testid={`lane-${id}`}>
      <rect
        x={0}
        y={0}
        width={width}
        height={LANE_H}
        fill={selected ? 'var(--accent)' : highlighted ? 'var(--muted)' : 'transparent'}
        stroke="none"
      />
      <line x1={0} x2={width} y1={LANE_H - 0.5} y2={LANE_H - 0.5} stroke="var(--border)" strokeWidth={1} />

      {clusters.map((c) => {
        const bad = c.failed
        return (
          <line
            key={c.bucket}
            data-tip="tick"
            data-lane={id}
            data-i={c.index}
            data-n={c.count}
            x1={c.x}
            x2={c.x}
            y1={TICK_Y1}
            y2={TICK_Y2}
            stroke={bad ? 'var(--destructive)' : 'var(--muted-foreground)'}
            strokeWidth={c.count > 1 ? 2.2 : 1.2}
            strokeOpacity={c.pending ? 0.5 : 1}
            strokeLinecap="round"
          />
        )
      })}

      {lane.segments.slice(sFrom, sTo).map((s, k) => {
        const i = sFrom + k
        if (s.end < t0 - padT || s.start > t1 + padT) return null
        const style = STATE_STYLES[s.state]
        const xs = clampX(x(s.start), width)
        const xe = clampX(x(s.end), width)
        const w = Math.max(xe - xs, s.end === s.start ? 0 : 1)
        const label = style.label
        const acct = accountAt(lane, s.start)
        const text = w > 190 && acct ? `${label} · ${acct}` : label
        const showText = w >= text.length * 6 + 10
        return (
          <g key={i} data-tip="segment" data-lane={id} data-i={i} className="cursor-pointer" data-testid={`seg-${id}-${i}`}>
            {w > 0 && (
              <>
                <StateFill state={s.state} x={xs} y={BAR_Y} w={w} h={BAR_H} />
                {showText && (
                  <text x={xs + 5} y={BAR_Y + BAR_H / 2 + 3.5} fontSize={10} fontWeight={600} fill={stateInkVar(s.state)} pointerEvents="none" style={{ paintOrder: 'stroke' }}>
                    {text}
                  </text>
                )}
              </>
            )}
            {i === activeSegment && w > 0 && (
              <rect x={xs - 1.5} y={BAR_Y - 1.5} width={w + 3} height={BAR_H + 3} rx={3} fill="none" stroke="var(--ring)" strokeWidth={2} pointerEvents="none" />
            )}
            {style.endGlyph && <EndGlyph glyph={style.endGlyph} x={xe} fill={stateFill(s.state)} ink={stateInkVar(s.state)} />}
            {/* A 3 px minimum hit target for very short segments. */}
            {w < 3 && w > 0 && <rect x={xs - 1.5} y={BAR_Y} width={w + 3} height={BAR_H} fill="transparent" />}
          </g>
        )
      })}

      {lane.attempts.slice(aFrom, aTo).map((a, k) => {
        const i = aFrom + k
        if (a.end < t0 - padT || a.start > t1 + padT) return null
        const xs = clampX(x(a.start), width)
        const xe = clampX(x(a.end), width)
        return (
          <rect key={i} data-tip="attempt" data-lane={id} data-i={i} x={xs} y={ATTEMPT_Y} width={Math.max(xe - xs - 1, 1)} height={ATTEMPT_H} rx={1} fill={ATTEMPT_FILL[a.outcome]} opacity={0.9} className="cursor-pointer" />
        )
      })}

      {lane.spans.map((sp, i) => {
        if (sp.end < t0 - padT || sp.start > t1 + padT) return null
        const xs = clampX(x(sp.start), width)
        const xe = clampX(x(sp.end), width)
        const top = BAR_Y - 2
        const bot = BAR_Y + BAR_H + 2
        return sp.kind === 'question' ? (
          <rect key={i} data-tip="span" data-lane={id} data-i={i} x={xs} y={top} width={Math.max(xe - xs, 2)} height={bot - top} rx={3} fill="none" stroke="var(--state-pending-review)" strokeWidth={1.4} strokeDasharray="4 3" pointerEvents="stroke" />
        ) : (
          <path key={i} data-tip="span" data-lane={id} data-i={i} d={`M${xs + 3} ${top} H${xs} V${bot} H${xs + 3} M${xe - 3} ${top} H${xe} V${bot} H${xe - 3}`} fill="none" stroke="var(--foreground)" strokeWidth={1.6} pointerEvents="stroke" />
        )
      })}

      {lane.markers.slice(mFrom, mTo).map((m, k) => {
        const i = mFrom + k
        return (
          <g key={i} data-tip="marker" data-lane={id} data-i={i} className="cursor-pointer">
            <MarkerGlyph kind={m.kind} ok={m.ok} x={x(m.at)} y={BAR_Y + BAR_H / 2} />
          </g>
        )
      })}
    </g>
  )
})
