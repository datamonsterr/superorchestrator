import type { RunState } from '@/api/generated'
import type { Event } from '@/api/types'
import {
  asString,
  isTerminalState,
  ORCHESTRATOR_LANE,
  type Arrow,
  type AttemptSegment,
  type RunLane,
  type StateSegment,
  type TimelineModel,
} from './model'

const STATES: ReadonlySet<string> = new Set<RunState>([
  'queued', 'starting', 'running', 'waiting', 'stalled', 'escalated', 'pending_review', 'succeeded', 'failed', 'cancelled',
])
const asState = (v: unknown): RunState | null => (typeof v === 'string' && STATES.has(v) ? (v as RunState) : null)
const str = (d: Record<string, unknown>, k: string): string | undefined => asString(d[k])

/** Compares `(ts, id)` keys. */
export function compareKeys(a: { ts: number; id: string }, b: { ts: number; id: string }): number {
  const byId = a.id < b.id ? -1 : a.id > b.id ? 1 : 0
  const an = Number.isNaN(a.ts)
  const bn = Number.isNaN(b.ts)
  if (an || bn) return an && bn ? byId : an ? -1 : 1 // an unparseable time sorts first, keeping the order total
  return a.ts - b.ts || byId
}

export const eventMs = (ev: Event): number => Date.parse(ev.ts)

export function cloneLane(l: RunLane): RunLane {
  const pendingTools: Record<string, number[]> = {}
  for (const k in l.pendingTools) pendingTools[k] = l.pendingTools[k].slice()
  return {
    ...l,
    segments: l.segments.slice(),
    attempts: l.attempts.slice(),
    ticks: l.ticks.slice(),
    markers: l.markers.slice(),
    spans: l.spans.slice(),
    pendingTools,
    openQuestions: l.openQuestions.slice(),
    openJoins: l.openJoins.slice(),
    cur: l.cur ? { ...l.cur } : null,
  }
}

function lastSeg(l: RunLane): StateSegment | undefined {
  return l.segments[l.segments.length - 1]
}

function openState(l: RunLane, state: RunState, ms: number) {
  const terminal = isTerminalState(state)
  l.segments.push({ kind: 'state', runId: l.runId, state, start: ms, end: ms, open: !terminal })
  l.cur = { state, start: ms }
  l.terminal = terminal
}

/** Closes the open state segment at `ms`. */
function closeCur(l: RunLane, ms: number) {
  const seg = lastSeg(l)
  if (l.cur && seg) l.segments[l.segments.length - 1] = { ...seg, end: Math.max(ms, seg.start), open: false }
  l.cur = null
}

function openAttempt(l: RunLane): AttemptSegment | undefined {
  const a = l.attempts[l.attempts.length - 1]
  return a && a.outcome === 'running' ? a : undefined
}
function setLastAttempt(l: RunLane, a: AttemptSegment) {
  l.attempts[l.attempts.length - 1] = a
}

/**
 * Applies one event to lane `l` (which the caller owns) and to the shared `m.arrows`.
 * Mutates; `applyEvent` and `deriveTimeline` take care of copying first.
 */
export function applyCore(m: TimelineModel, l: RunLane, ev: Event, ms: number) {
  const d: Record<string, unknown> = ev.data && typeof ev.data === 'object' ? (ev.data as Record<string, unknown>) : {}
  l.eventCount++
  switch (ev.type) {
    case 'run_queued':
      if (!l.cur) openState(l, 'queued', ms)
      break
    case 'run_started':
      if (!l.cur) openState(l, 'starting', ms)
      break
    case 'state_changed': {
      const to = asState(d.to)
      if (!to) break
      const from = asState(d.from)
      if (l.cur && from && l.cur.state !== from) m.warnings++ // trust the event, insert nothing
      closeCur(l, ms)
      openState(l, to, ms)
      break
    }
    case 'run_finished': {
      const status = asState(d.status)
      const seg = lastSeg(l)
      if (l.cur && seg) l.segments[l.segments.length - 1] = { ...seg, end: Math.max(ms, seg.start), open: false }
      l.cur = null
      const tail = lastSeg(l)
      if (status && isTerminalState(status) && (!tail || !isTerminalState(tail.state))) {
        l.segments.push({ kind: 'state', runId: l.runId, state: status, start: ms, end: ms, open: false })
      }
      l.terminal = true
      const a = openAttempt(l)
      if (a) setLastAttempt(l, { ...a, end: ms, outcome: status === 'succeeded' ? 'ok' : 'failed' })
      break
    }
    case 'attempt_started': {
      const account = str(d, 'account') ?? 'unknown'
      const a = openAttempt(l)
      if (a && a.implicit && a.account === account) {
        setLastAttempt(l, { ...a, implicit: false })
        break
      }
      if (a) {
        if (a.implicit) l.attempts.pop()
        else setLastAttempt(l, { ...a, end: ms, outcome: 'failed' })
      }
      l.attempts.push({ kind: 'attempt', runId: l.runId, account, start: ms, end: ms, outcome: 'running' })
      break
    }
    case 'failover': {
      const a = openAttempt(l)
      const reason = str(d, 'reason')
      if (a) setLastAttempt(l, { ...a, end: ms, outcome: 'failover', ...(reason ? { reason } : {}) })
      const to = str(d, 'to')
      if (to) l.attempts.push({ kind: 'attempt', runId: l.runId, account: to, start: ms, end: ms, outcome: 'running', implicit: true })
      break
    }
    case 'tool_call_started': {
      const name = str(d, 'name') ?? 'tool'
      l.ticks.push({ kind: 'tool', runId: l.runId, at: ms, name, ok: null })
      ;(l.pendingTools[name] ??= []).push(l.ticks.length - 1)
      break
    }
    case 'tool_call_finished': {
      const name = str(d, 'name') ?? 'tool'
      const ok = d.ok !== false
      const idx = l.pendingTools[name]?.shift()
      if (idx !== undefined) l.ticks[idx] = { ...l.ticks[idx], ok }
      else l.ticks.push({ kind: 'tool', runId: l.runId, at: ms, name, ok })
      break
    }
    case 'stall_detected': {
      const idle = typeof d.idle_ms === 'number' ? ` (idle ${Math.round(d.idle_ms / 1000)}s)` : ''
      l.markers.push({ kind: 'stall', runId: l.runId, at: ms, label: `Stall detected${idle}` })
      break
    }
    case 'nudge_sent':
      l.markers.push({ kind: 'nudge', runId: l.runId, at: ms, label: 'Nudge sent' })
      break
    case 'escalated':
      l.markers.push({ kind: 'escalated', runId: l.runId, at: ms, label: str(d, 'reason') ? `Escalated: ${str(d, 'reason')}` : 'Escalated' })
      break
    case 'question_asked': {
      const label = str(d, 'question') ?? str(d, 'text') ?? 'Question'
      l.markers.push({ kind: 'question', runId: l.runId, at: ms, label })
      l.spans.push({ kind: 'question', runId: l.runId, start: ms, end: ms, open: true, label })
      l.openQuestions.push(l.spans.length - 1)
      break
    }
    case 'question_answered': {
      const idx = l.openQuestions.shift()
      if (idx !== undefined) l.spans[idx] = { ...l.spans[idx], end: ms, open: false }
      break
    }
    case 'check_result': {
      const ok = d.ok !== false
      const name = str(d, 'name') ?? str(d, 'check')
      l.markers.push({ kind: 'check', runId: l.runId, at: ms, ok, label: `Check${name ? ` ${name}` : ''} ${ok ? 'passed' : 'failed'}` })
      break
    }
    case 'review_accepted':
      l.markers.push({ kind: 'review', runId: l.runId, at: ms, ok: true, label: 'Review accepted' })
      break
    case 'review_rejected':
      l.markers.push({ kind: 'review', runId: l.runId, at: ms, ok: false, label: str(d, 'feedback') ? `Review rejected: ${str(d, 'feedback')}` : 'Review rejected' })
      break
    case 'join_command_issued':
      l.markers.push({ kind: 'join', runId: l.runId, at: ms, label: 'Join command issued' })
      break
    case 'user_joined':
      l.spans.push({ kind: 'joined', runId: l.runId, start: ms, end: ms, open: true, label: 'A person is attached' })
      l.openJoins.push(l.spans.length - 1)
      break
    case 'user_left': {
      const idx = l.openJoins.shift()
      if (idx !== undefined) l.spans[idx] = { ...l.spans[idx], end: ms, open: false }
      break
    }
    case 'message_sent': {
      const to = str(d, 'to')
      if (to) m.arrows.push({ kind: 'message', from: ev.run_id ?? ORCHESTRATOR_LANE, to, at: ms, msgKind: str(d, 'kind') ?? 'message' })
      break
    }
    default:
      break // usage, assistant_message, progress, route_decision, worktree_*: not drawn
  }
}

function closeFromRunView(l: RunLane, state: RunState, ft: number): RunLane {
  const c = cloneLane(l)
  closeCur(c, ft)
  c.segments.push({ kind: 'state', runId: c.runId, state, start: ft, end: ft, open: false })
  const att = openAttempt(c)
  if (att) setLastAttempt(c, { ...att, end: ft, outcome: state === 'succeeded' ? 'ok' : 'failed' })
  c.terminal = true
  return c
}

/**
 * Re-ends open segments and attempts at `now` and recomputes `tMin`/`tMax`.
 * Returns the same model when nothing changed; otherwise only lanes with an open
 * end are copied.
 */
export function refreshNow(m: TimelineModel, now: number): TimelineModel {
  let lanes: RunLane[] | null = null
  let active = false
  for (let i = 0; i < m.lanes.length; i++) {
    let l = m.lanes[i]
    // The run view says it finished but no terminal event was seen (partial history): close it there.
    if (!l.terminal && l.cur && l.run && isTerminalState(l.run.state) && l.run.finished_at) {
      const ft = Date.parse(l.run.finished_at)
      // Only once events up to that time were applied, so a fold and a derive agree on every prefix.
      if (ft >= l.cur.start && m.lastKey !== null && m.lastKey.ts >= ft) {
        l = closeFromRunView(l, l.run.state, ft)
        lanes ??= m.lanes.slice()
        lanes[i] = l
      }
    }
    const live = !l.terminal && (l.eventCount > 0 || !l.run || !isTerminalState(l.run.state)) && l.runId !== ORCHESTRATOR_LANE
    if (live) active = true
    const seg = lastSeg(l)
    const att = l.attempts[l.attempts.length - 1]
    const segStale = !!(live && l.cur && seg && seg.open && seg.end !== now)
    const attStale = !!(live && att && att.outcome === 'running' && att.end !== now)
    if (!segStale && !attStale) continue
    lanes ??= m.lanes.slice()
    const next = { ...l }
    if (segStale) {
      next.segments = l.segments.slice()
      next.segments[next.segments.length - 1] = { ...seg!, end: Math.max(now, seg!.start) }
    }
    if (attStale) {
      next.attempts = l.attempts.slice()
      next.attempts[next.attempts.length - 1] = { ...att!, end: Math.max(now, att!.start) }
    }
    lanes[i] = next
  }
  // Bounds.
  let tMin = m.firstTs ?? Number.POSITIVE_INFINITY
  let latest = m.lastKey?.ts ?? Number.NEGATIVE_INFINITY
  for (const l of m.lanes) {
    const c = l.run?.created_at ? Date.parse(l.run.created_at) : NaN
    if (c < tMin) tMin = c
    const f = l.run?.finished_at ? Date.parse(l.run.finished_at) : NaN
    if (f > latest) latest = f
  }
  if (!Number.isFinite(tMin)) tMin = now
  let tMax = active ? now : Number.isFinite(latest) ? latest : tMin
  if (tMax <= tMin) tMax = tMin + 1000
  if (!lanes && m.active === active && m.tMin === tMin && m.tMax === tMax && (m.now === now || !active)) return m
  return { ...m, lanes: lanes ?? m.lanes, now, tMin, tMax, active }
}

/**
 * Returns a new model with `event` applied (live append). Only the touched lane
 * (and `arrows` for messages) get new objects. An event that sorts before
 * `lastKey` is not applied: the result has `needsRebuild` set and the caller
 * re-derives from the raw events. Events of unknown runs are listed in
 * `missingRuns`.
 */
export function applyEvent(model: TimelineModel, event: Event, now: number): TimelineModel {
  const ms = eventMs(event)
  if (!Number.isFinite(ms)) return model
  const key = { ts: ms, id: event.id }
  if (model.lastKey && compareKeys(key, model.lastKey) < 0) return { ...model, needsRebuild: true }
  const laneId = event.run_id ?? ORCHESTRATOR_LANE
  const idx = model.index[laneId]
  if (idx === undefined) {
    const missing = model.missingRuns.includes(laneId) ? model.missingRuns : [...model.missingRuns, laneId]
    return refreshNow({ ...model, missingRuns: missing, lastKey: key, firstTs: model.firstTs ?? ms }, now)
  }
  const lane = cloneLane(model.lanes[idx])
  const lanes = model.lanes.slice()
  lanes[idx] = lane
  const next: TimelineModel = {
    ...model,
    lanes,
    arrows: event.type === 'message_sent' ? model.arrows.slice() : model.arrows,
    lastKey: key,
    firstTs: model.firstTs ?? ms,
  }
  applyCore(next, lane, event, ms)
  return refreshNow(next, now)
}



/**
 * Applies a batch in order: each touched lane is cloned once and bounds are refreshed once. Equal to folding
 * `applyEvent`. Stops at the first event that sorts before `lastKey` (`needsRebuild`).
 */
export function applyEvents(model: TimelineModel, events: readonly Event[], now: number): TimelineModel {
  let lanes: RunLane[] | null = null
  const owned = new Set<number>()
  let arrows: Arrow[] | null = null
  let m: TimelineModel = model
  for (const event of events) {
    const ms = eventMs(event)
    if (!Number.isFinite(ms)) continue
    const key = { ts: ms, id: event.id }
    if (m.lastKey && compareKeys(key, m.lastKey) < 0) {
      m = { ...m, lanes: lanes ?? m.lanes, arrows: arrows ?? m.arrows, needsRebuild: true }
      return refreshNow(m, now)
    }
    const laneId = event.run_id ?? ORCHESTRATOR_LANE
    const idx = m.index[laneId]
    m = { ...m, lastKey: key, firstTs: m.firstTs ?? ms }
    if (idx === undefined) {
      if (!m.missingRuns.includes(laneId)) m.missingRuns = [...m.missingRuns, laneId]
      continue
    }
    lanes ??= m.lanes.slice()
    if (!owned.has(idx)) {
      lanes[idx] = cloneLane(lanes[idx])
      owned.add(idx)
    }
    if (event.type === 'message_sent' && !arrows) arrows = m.arrows.slice()
    m.lanes = lanes
    if (arrows) m.arrows = arrows
    applyCore(m, lanes[idx], event, ms)
  }
  return refreshNow(m, now)
}
