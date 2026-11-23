import type { RunLane, StateSegment, TimelineModel } from './model'
import { isTerminalState, ORCHESTRATOR_LANE } from './model'
import { STATE_STYLES } from './colors'
import { formatDuration } from '@/lib/format'
import { formatClock } from '@/lib/time'

/** Geometry (design 4.4): 32 px lanes, 14 px bars, 4 px attempt bars, ticks above, 12 px markers on the bar. */
export const LANE_H = 32
export const TICK_Y1 = 1
export const TICK_Y2 = 7
export const BAR_Y = 10
export const BAR_H = 14
export const ATTEMPT_Y = 27
export const ATTEMPT_H = 4
export const GLYPH = 12
export const AXIS_H = 28
/** Lanes are virtualised only above this count. */
export const VIRTUALISE_ABOVE = 60
export const MAX_ARROWS = 200

export function laneTitle(lane: RunLane): string {
  if (lane.runId === ORCHESTRATOR_LANE) return 'Orchestrator'
  return lane.run?.name ?? lane.runId
}

export function laneCurrentState(lane: RunLane): StateSegment['state'] | undefined {
  const lastSeg = lane.segments[lane.segments.length - 1]
  const lastState = lastSeg?.state
  const snapshotState = lane.run?.state
  if (!snapshotState) return lastState
  if (!lastState) return snapshotState
  if (lastState === snapshotState) return lastState

  // 1. Authoritative snapshot state when history is incomplete:
  // A run is never in queued or starting once its snapshot has reached an advanced state
  // (such as pending_review, running, stalled, escalated, succeeded, failed, cancelled).
  if ((lastState === 'queued' || lastState === 'starting') && snapshotState !== lastState) {
    return snapshotState
  }

  // 2. Terminal snapshot states are authoritative over non-terminal event states:
  if (isTerminalState(snapshotState) && !isTerminalState(lastState)) {
    return snapshotState
  }

  // 3. If snapshot is pending_review, check whether any event was emitted after entering pending_review.
  // When replayed events omitted later transitions, earlier state events (with later markers on lane)
  // defer to the authoritative snapshot.
  if (snapshotState === 'pending_review' && lastState !== 'pending_review') {
    let latestActivity = lastSeg.start
    for (const m of lane.markers) if (m.at > latestActivity) latestActivity = m.at
    for (const t of lane.ticks) if (t.at > latestActivity) latestActivity = t.at
    if (latestActivity > lastSeg.start) {
      return snapshotState
    }
  }

  // 4. Otherwise, newer live transitions from events win.
  return lastState
}

/** Account the lane was running on at time `t` (the attempt covering it, else the latest started). */
export function accountAt(lane: RunLane, t: number): string | undefined {
  let found: string | undefined
  for (const a of lane.attempts) {
    if (a.start <= t) found = a.account
    else break
  }
  return found ?? lane.run?.account ?? undefined
}

const span = (a: number, b: number) => `${formatClock(a, true)} to ${formatClock(b, true)}`

/** Screen-reader sentence for a state segment: "running, 12:01 to 12:04, codex-acc1". */
export function describeSegment(lane: RunLane, seg: StateSegment): string {
  const acct = accountAt(lane, seg.start)
  return `${STATE_STYLES[seg.state].label.toLowerCase()}, ${formatClock(seg.start)} to ${formatClock(seg.end)}${acct ? `, ${acct}` : ''}`
}

/** Tooltip lines for a hovered element, from its `data-*` attributes. */
export function describeTip(model: TimelineModel, el: Element): string[] | null {
  const d = (el as HTMLElement).dataset
  const kind = d.tip
  if (!kind) return null
  if (kind === 'arrow') {
    const a = model.arrows[Number(d.i)]
    if (!a) return null
    const name = (id: string) => (id === ORCHESTRATOR_LANE ? 'orchestrator' : (model.lanes[model.index[id]]?.run?.name ?? id))
    return [`Message (${a.msgKind})`, `${name(a.from)} to ${name(a.to)}`, formatClock(a.at, true)]
  }
  const lane = model.lanes[model.index[d.lane ?? '']]
  if (!lane) return null
  const i = Number(d.i)
  switch (kind) {
    case 'segment': {
      const s = lane.segments[i]
      if (!s) return null
      const acct = accountAt(lane, s.start)
      return [
        `${STATE_STYLES[s.state].label}${s.open ? ' (now)' : ''}`,
        `${span(s.start, s.end)} (${formatDuration(s.end - s.start)})`,
        ...(acct ? [`Account ${acct}`] : []),
      ]
    }
    case 'attempt': {
      const a = lane.attempts[i]
      if (!a) return null
      return [
        `Attempt on ${a.account}`,
        `${a.outcome}${a.reason ? ` (${a.reason})` : ''}`,
        `${span(a.start, a.end)} (${formatDuration(a.end - a.start)})`,
      ]
    }
    case 'tick': {
      const t = lane.ticks[i]
      if (!t) return null
      const n = Number(d.n) || 1
      if (n > 1) return [`${n} tool calls`, `starting ${formatClock(t.at, true)}`, 'Zoom in to split']
      return [t.name, t.ok === null ? 'running' : t.ok ? 'ok' : 'failed', formatClock(t.at, true)]
    }
    case 'marker': {
      const m = lane.markers[i]
      return m ? [m.label, formatClock(m.at, true)] : null
    }
    case 'span': {
      const s = lane.spans[i]
      return s ? [s.label, `${span(s.start, s.end)}${s.open ? ' (open)' : ''}`] : null
    }
    default:
      return null
  }
}
