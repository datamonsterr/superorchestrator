import { memo } from 'react'
import type { MarkerKind } from './model'
import type { RunState } from '@/api/types'
import { patternId, stateFill, STATE_STYLES } from './colors'
import { BAR_H, BAR_Y, GLYPH, LANE_H } from './layout'

const STROKE = { stroke: 'var(--foreground)', strokeWidth: 1.3, fill: 'none', strokeLinecap: 'round', strokeLinejoin: 'round' } as const

function Shape({ kind, ok }: { kind: MarkerKind; ok?: boolean }) {
  switch (kind) {
    case 'stall':
      return (
        <g {...STROKE}>
          <path d="M6 2.4 L9.8 9.2 H2.2 Z" />
          <path d="M6 5.2 V7" />
        </g>
      )
    case 'nudge':
      return <path {...STROKE} d="M2.5 6 H9 M6.5 3 L9.5 6 L6.5 9" />
    case 'escalated':
      return <path d="M6 1.8 L10.2 6 L6 10.2 L1.8 6 Z" fill="var(--foreground)" />
    case 'question':
      return (
        <text x="6" y="9.2" textAnchor="middle" fontSize="9" fontWeight="700" fill="var(--foreground)">
          ?
        </text>
      )
    case 'check':
      return ok === false ? <path {...STROKE} d="M3.6 3.6 L8.4 8.4 M8.4 3.6 L3.6 8.4" /> : <path {...STROKE} d="M3 6.2 L5.2 8.4 L9 3.8" />
    case 'review':
      return <path {...STROKE} fill={ok === false ? 'none' : 'var(--foreground)'} d="M4 2.5 V9.8 M4 2.8 H9 L7.5 4.8 L9 6.8 H4" />
    case 'join':
      return (
        <g {...STROKE}>
          <circle cx="6" cy="4.3" r="1.5" />
          <path d="M3 9.5 C3 7.4 9 7.4 9 9.5" />
        </g>
      )
  }
}

/** A 12 px marker badge centred on (x, y). Shapes differ per kind, so kind is never colour-only. */
export const MarkerGlyph = memo(function MarkerGlyph({ kind, ok, x, y }: { kind: MarkerKind; ok?: boolean; x: number; y: number }) {
  return (
    <g transform={`translate(${x - GLYPH / 2} ${y - GLYPH / 2})`}>
      <circle cx="6" cy="6" r="5.7" fill="var(--background)" stroke="var(--foreground)" strokeWidth="0.8" />
      <Shape kind={kind} ok={ok} />
    </g>
  )
})

/** Standalone glyph for the legend. */
export function MarkerIcon({ kind, ok, size = 16 }: { kind: MarkerKind; ok?: boolean; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 12 12" aria-hidden focusable="false">
      <circle cx="6" cy="6" r="5.7" fill="var(--background)" stroke="var(--foreground)" strokeWidth="0.8" />
      <Shape kind={kind} ok={ok} />
    </svg>
  )
}

/** The check or x drawn at the end of a finished run's bar. */
export const EndGlyph = memo(function EndGlyph({ glyph, x, fill, ink }: { glyph: 'check' | 'x'; x: number; fill: string; ink: string }) {
  const cy = BAR_Y + BAR_H / 2
  return (
    <g transform={`translate(${x - 6} ${cy - 6})`} pointerEvents="none">
      <circle cx="6" cy="6" r="6" fill={fill} stroke="var(--background)" strokeWidth="1" />
      <path
        d={glyph === 'check' ? 'M3 6.2 L5.2 8.4 L9 3.8' : 'M3.8 3.8 L8.2 8.2 M8.2 3.8 L3.8 8.2'}
        stroke={ink}
        strokeWidth="1.5"
        fill="none"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </g>
  )
})

/** Pattern overlays (one definition per svg). Ink comes from `--state-ink`. */
export function PatternDefs() {
  const ink = 'var(--state-ink)'
  return (
    <defs>
      <pattern id={patternId('dots')} width="6" height="6" patternUnits="userSpaceOnUse">
        <circle cx="3" cy="3" r="1.1" fill={ink} />
      </pattern>
      <pattern id={patternId('hatch')} width="6" height="6" patternUnits="userSpaceOnUse">
        <path d="M-1 7 L7 -1 M-1 1 L1 -1 M5 7 L7 5" stroke={ink} strokeWidth="1.2" />
      </pattern>
      <pattern id={patternId('cross')} width="6" height="6" patternUnits="userSpaceOnUse">
        <path d="M-1 7 L7 -1 M-1 -1 L7 7" stroke={ink} strokeWidth="1" />
      </pattern>
      <pattern id={patternId('stripe')} width="8" height={LANE_H} patternUnits="userSpaceOnUse">
        <rect x="0" y={BAR_Y + BAR_H / 2 - 1} width="8" height="2" fill={ink} />
      </pattern>
      <marker id="so-arrow-head" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0 0 L8 4 L0 8 Z" fill="var(--muted-foreground)" />
      </marker>
    </defs>
  )
}


/** The coloured bar of a state with its pattern cue (fill, overlay, dashed outline or strikethrough). */
export const StateFill = memo(function StateFill({ state, x, y, w, h, rx = 2 }: { state: RunState; x: number; y: number; w: number; h: number; rx?: number }) {
  const { pattern } = STATE_STYLES[state]
  return (
    <>
      <rect x={x} y={y} width={w} height={h} rx={rx} fill={stateFill(state)} />
      {(pattern === 'dots' || pattern === 'hatch' || pattern === 'cross' || pattern === 'stripe') && (
        <rect x={x} y={y} width={w} height={h} rx={rx} fill={`url(#${patternId(pattern)})`} pointerEvents="none" />
      )}
      {pattern === 'dashed' && (
        <rect x={x + 1} y={y + 1} width={Math.max(w - 2, 0)} height={Math.max(h - 2, 0)} rx={1.5} fill="none" stroke="var(--state-ink)" strokeWidth={1.2} strokeDasharray="3 2" pointerEvents="none" />
      )}
      {pattern === 'strike' && <line x1={x} x2={x + w} y1={y + h / 2} y2={y + h / 2} stroke="var(--state-ink)" strokeWidth={1.6} pointerEvents="none" />}
    </>
  )
})
