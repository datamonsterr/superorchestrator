import type { RunState, RunViewPublic } from '@/api/types'

/** Lane id of the synthetic top lane (events without a run, user joins, orchestrator messages). */
export const ORCHESTRATOR_LANE = 'orchestrator'

export const TERMINAL_STATES: ReadonlySet<RunState> = new Set<RunState>(['succeeded', 'failed', 'cancelled'])
export const isTerminalState = (s: RunState): boolean => TERMINAL_STATES.has(s)

/** Times are epoch milliseconds throughout. */
export interface StateSegment {
  kind: 'state'
  runId: string
  state: RunState
  start: number
  /** While `open` this is the `now` of the last derive / refresh. */
  end: number
  open: boolean
}
export type AttemptOutcome = 'ok' | 'failover' | 'failed' | 'running'
export interface AttemptSegment {
  kind: 'attempt'
  runId: string
  account: string
  start: number
  end: number
  outcome: AttemptOutcome
  reason?: string
  /** Opened by a `failover` event rather than by `attempt_started`. */
  implicit?: boolean
}
export type Segment = StateSegment | AttemptSegment

export interface Tick {
  kind: 'tool'
  runId: string
  at: number
  name: string
  /** null until the matching `tool_call_finished` arrives. */
  ok: boolean | null
}
export interface Arrow {
  kind: 'message'
  from: string
  to: string
  at: number
  msgKind: string
}
export type MarkerKind = 'stall' | 'nudge' | 'escalated' | 'question' | 'check' | 'review' | 'join'
export interface Marker {
  kind: MarkerKind
  runId: string
  at: number
  label: string
  ok?: boolean
}
/** A bracket on the lane: a question awaiting its answer, or a person attached to the run. */
export interface Span {
  kind: 'question' | 'joined'
  runId: string
  start: number
  end: number
  open: boolean
  label: string
}

export interface RunLane {
  runId: string
  /** Undefined for the orchestrator lane. */
  run?: RunViewPublic
  /** Nesting depth under the orchestrator lane; top-level runs are 0. */
  depth: number
  segments: StateSegment[]
  attempts: AttemptSegment[]
  ticks: Tick[]
  markers: Marker[]
  spans: Span[]
  /** The lane has ended (terminal state or `run_finished`). */
  terminal: boolean
  /** Number of events applied to this lane. */
  eventCount: number
  /** Internal bookkeeping for incremental apply. */
  cur: { state: RunState; start: number } | null
  pendingTools: Record<string, number[]>
  openQuestions: number[]
  openJoins: number[]
}

export interface TimelineModel {
  lanes: RunLane[]
  /** runId -> index into `lanes`. */
  index: Record<string, number>
  arrows: Arrow[]
  tMin: number
  tMax: number
  now: number
  /** Any run lane is still live, so `tMax` follows `now`. */
  active: boolean
  lastKey: { ts: number; id: string } | null
  /** Earliest event time applied (internal, feeds `tMin`). */
  firstTs: number | null
  /** `state_changed.from` disagreements, surfaced in the dev console only. */
  warnings: number
  /** Run ids seen in events that have no lane yet: the caller should refetch the session. */
  missingRuns: string[]
  /** Set when an event sorted before `lastKey`: the caller must re-derive from the raw events. */
  needsRebuild: boolean
}
