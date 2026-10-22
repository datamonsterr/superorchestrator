import type { RunState } from '@/api/types'
import type { MarkerKind } from './model'
import { readableInk } from '@/lib/contrast'

/**
 * Gantt colours (design 4.3). The hex values here are the single source of truth for the contrast test
 * and are mirrored as CSS variables in `timeline.css` (a test fails on drift). Colour is a pure function of
 * `RunState`; every state also has a pattern or glyph so it is never the only signal.
 */
export type PatternKind = 'dots' | 'hatch' | 'solid' | 'stripe' | 'cross' | 'dashed' | 'strike'

export interface StateStyle {
  label: string
  light: string
  dark: string
  pattern: PatternKind
  patternLabel: string
  /** Glyph drawn at the end of a finished segment. */
  endGlyph?: 'check' | 'x'
}

export const STATE_ORDER: RunState[] = [
  'queued', 'starting', 'running', 'waiting', 'stalled', 'escalated', 'pending_review', 'succeeded', 'failed', 'cancelled',
]

/**
 * Deviation from design 4.3: `queued` and `starting` light fills were `#94a3b8` and `#38bdf8`, which are 2.56:1
 * and 2.14:1 against white and fail the 3:1 rule in the same section; they are darkened to pass.
 */
export const STATE_STYLES: Record<RunState, StateStyle> = {
  queued: { label: 'Queued', light: '#64748b', dark: '#64748b', pattern: 'dots', patternLabel: 'dotted' },
  starting: { label: 'Starting', light: '#0284c7', dark: '#0ea5e9', pattern: 'hatch', patternLabel: 'diagonal hatch' },
  running: { label: 'Running', light: '#16a34a', dark: '#22c55e', pattern: 'solid', patternLabel: 'solid' },
  waiting: { label: 'Waiting', light: '#2563eb', dark: '#60a5fa', pattern: 'stripe', patternLabel: 'inner stripe' },
  stalled: { label: 'Stalled', light: '#d97706', dark: '#fbbf24', pattern: 'cross', patternLabel: 'cross hatch' },
  escalated: { label: 'Escalated', light: '#dc2626', dark: '#f87171', pattern: 'cross', patternLabel: 'cross hatch and marker' },
  pending_review: { label: 'Pending review', light: '#9333ea', dark: '#c084fc', pattern: 'dashed', patternLabel: 'dashed outline' },
  succeeded: { label: 'Succeeded', light: '#0f766e', dark: '#2dd4bf', pattern: 'solid', patternLabel: 'solid, check at end', endGlyph: 'check' },
  failed: { label: 'Failed', light: '#991b1b', dark: '#ef4444', pattern: 'solid', patternLabel: 'solid, x at end', endGlyph: 'x' },
  cancelled: { label: 'Cancelled', light: '#52525b', dark: '#a1a1aa', pattern: 'strike', patternLabel: 'strikethrough' },
}

/** Lane background the fills must stand out from (`--background`). */
export const LANE_BG = { light: '#ffffff', dark: '#0a0a0a' } as const

export const stateVarName = (s: RunState) => `--state-${s.replace('_', '-')}`
export const stateFill = (s: RunState) => `var(${stateVarName(s)})`
export const stateInkVar = (s: RunState) => `var(${stateVarName(s)}-ink)`
export const patternId = (k: PatternKind) => `so-pat-${k}`

/** Ink for text drawn on a segment, chosen per fill and theme. */
export const segmentInk = (s: RunState, theme: 'light' | 'dark') => readableInk(STATE_STYLES[s][theme])

export interface MarkerStyle {
  label: string
  description: string
}
export const MARKER_ORDER: MarkerKind[] = ['stall', 'nudge', 'escalated', 'question', 'check', 'review', 'join']
export const MARKER_STYLES: Record<MarkerKind, MarkerStyle> = {
  stall: { label: 'Stall', description: 'Stall detected' },
  nudge: { label: 'Nudge', description: 'Nudge sent' },
  escalated: { label: 'Escalated', description: 'Escalated to the orchestrator' },
  question: { label: 'Question', description: 'Question asked' },
  check: { label: 'Check', description: 'Check result' },
  review: { label: 'Review', description: 'Review accepted or rejected' },
  join: { label: 'Join', description: 'Join command issued' },
}

export const LEGEND = {
  states: STATE_ORDER.map((state) => ({ state, ...STATE_STYLES[state] })),
  markers: MARKER_ORDER.map((kind) => ({ kind, ...MARKER_STYLES[kind] })),
  spans: [
    { kind: 'question' as const, label: 'Question span', description: 'Asked, not yet answered' },
    { kind: 'joined' as const, label: 'Person attached', description: 'Between user joined and user left' },
  ],
}

/**
 * The `--state-*` custom properties for `:root` and `.dark` (design 4.3), generated from the table above so the
 * contrast test and the rendered CSS cannot drift. `GanttCanvas` injects it as a hoisted `<style>`.
 */
export function stateCss(): string {
  const block = (theme: 'light' | 'dark', ink: string) =>
    STATE_ORDER.map((s) => `${stateVarName(s)}:${STATE_STYLES[s][theme]};${stateVarName(s)}-ink:${segmentInk(s, theme)};`).join('') + `--state-ink:${ink};`
  return `:root{${block('light', 'rgb(255 255 255 / 0.6)')}}.dark{${block('dark', 'rgb(0 0 0 / 0.45)')}}`
}
export const STATE_CSS = stateCss()
