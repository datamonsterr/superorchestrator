import { describe, expect, it } from 'vitest'
import type { Event, EventType, RunViewPublic } from '@/api/types'
import { makeLargeSession } from '@/test/fixtures/large'
import { applyEvent } from './applyEvent'
import { deriveTimeline } from './derive'
import { ORCHESTRATOR_LANE, type RunLane, type TimelineModel } from './model'

const T0 = Date.UTC(2026, 0, 1, 12, 0, 0)
const iso = (s: number) => new Date(T0 + s * 1000).toISOString()
let n = 0
const ev = (type: EventType, run: string | null, s: number, data: unknown = {}, id?: string): Event => ({
  id: id ?? `e${String(++n).padStart(5, '0')}`,
  session_id: 's',
  run_id: run,
  ts: iso(s),
  type,
  data,
})
const run = (id: string, extra: Partial<RunViewPublic> = {}): RunViewPublic => ({
  run_id: id, state: 'running', recent_events: [], usage: { input_tokens: 0, output_tokens: 0, cost: 0 }, depth: 0, created_at: iso(0), ...extra,
})
const lane = (m: TimelineModel, id: string): RunLane => m.lanes[m.index[id]]
const NOW = T0 + 100_000

/** What a lane draws, with time as seconds from T0. */
const segs = (l: RunLane) => l.segments.map((s) => [s.state, (s.start - T0) / 1000, (s.end - T0) / 1000, s.open] as const)

describe('deriveTimeline', () => {
  it('derives a single run lifecycle and closes the lane at run_finished', () => {
    const events = [
      ev('run_queued', 'a', 0), ev('state_changed', 'a', 1, { from: 'queued', to: 'starting' }), ev('run_started', 'a', 2),
      ev('state_changed', 'a', 3, { from: 'starting', to: 'running' }),
      ev('state_changed', 'a', 20, { from: 'running', to: 'succeeded' }), ev('run_finished', 'a', 21, { status: 'succeeded' }),
    ]
    const m = deriveTimeline([run('a', { state: 'succeeded' })], events, NOW)
    const l = lane(m, 'a')
    expect(segs(l)).toEqual([
      ['queued', 0, 1, false], ['starting', 1, 3, false], ['running', 3, 20, false], ['succeeded', 20, 21, false],
    ])
    expect(l.terminal).toBe(true)
    expect(m.lanes[0].runId).toBe(ORCHESTRATOR_LANE)
    expect(m.tMin).toBe(T0)
    expect(m.tMax).toBe(T0 + 21_000) // no active run: latest event, not now
  })

  it('handles waiting and stalled sequences', () => {
    const events = [
      ev('run_started', 'a', 0), ev('state_changed', 'a', 5, { from: 'starting', to: 'running' }),
      ev('state_changed', 'a', 10, { from: 'running', to: 'waiting' }),
      ev('state_changed', 'a', 15, { from: 'waiting', to: 'running' }),
      ev('state_changed', 'a', 30, { from: 'running', to: 'stalled' }), ev('stall_detected', 'a', 30, { idle_ms: 60000 }),
    ]
    const m = deriveTimeline([run('a')], events, NOW)
    expect(segs(lane(m, 'a')).map((s) => s[0])).toEqual(['starting', 'running', 'waiting', 'running', 'stalled'])
    expect(lane(m, 'a').markers).toMatchObject([{ kind: 'stall', label: 'Stall detected (idle 60s)' }])
    expect(m.warnings).toBe(0)
  })

  it('ends the open segment at now while the run is active, and not after it is terminal', () => {
    const m = deriveTimeline([run('a')], [ev('run_started', 'a', 0)], NOW)
    expect(segs(lane(m, 'a'))).toEqual([['starting', 0, 100, true]])
    expect(m.tMax).toBe(NOW)
    const later = applyEvent(m, ev('run_finished', 'a', 50, { status: 'cancelled' }), NOW + 50_000)
    expect(lane(later, 'a').segments.every((s) => !s.open)).toBe(true)
    expect(later.tMax).toBe(T0 + 50_000)
  })

  it('records a failover across two attempts', () => {
    const events = [
      ev('attempt_started', 'a', 0, { account: 'codex-acc1' }),
      ev('failover', 'a', 10, { from: 'codex-acc1', to: 'codex-acc2', reason: 'quota' }),
      ev('attempt_started', 'a', 10, { account: 'codex-acc2' }),
      ev('run_finished', 'a', 30, { status: 'succeeded' }),
    ]
    const l = lane(deriveTimeline([run('a')], events, NOW), 'a')
    expect(l.attempts.map((a) => [a.account, a.outcome, a.reason, a.end - a.start])).toEqual([
      ['codex-acc1', 'failover', 'quota', 10_000],
      ['codex-acc2', 'ok', undefined, 20_000],
    ])
  })

  it('opens the next attempt itself when failover is not followed by attempt_started', () => {
    const events = [
      ev('attempt_started', 'a', 0, { account: 'x1' }),
      ev('failover', 'a', 10, { from: 'x1', to: 'x2', reason: 'rate_limit' }),
      ev('run_finished', 'a', 30, { status: 'failed' }),
    ]
    const l = lane(deriveTimeline([run('a')], events, NOW), 'a')
    expect(l.attempts.map((a) => [a.account, a.outcome])).toEqual([['x1', 'failover'], ['x2', 'failed']])
  })

  it('pairs tool calls FIFO per name and keeps unmatched finishes', () => {
    const events = [
      ev('tool_call_started', 'a', 1, { name: 'bash' }), ev('tool_call_started', 'a', 2, { name: 'bash' }),
      ev('tool_call_finished', 'a', 3, { name: 'bash', ok: false }), ev('tool_call_finished', 'a', 4, { name: 'bash', ok: true }),
      ev('tool_call_started', 'a', 5, { name: 'grep' }),
      ev('tool_call_finished', 'a', 6, { name: 'edit', ok: true }),
    ]
    const ticks = lane(deriveTimeline([run('a')], events, NOW), 'a').ticks
    expect(ticks.map((t) => [t.name, (t.at - T0) / 1000, t.ok])).toEqual([
      ['bash', 1, false], ['bash', 2, true], ['grep', 5, null], ['edit', 6, true],
    ])
  })

  it('draws message arrows from the sender, or the orchestrator when there is none', () => {
    const events = [ev('message_sent', 'a', 4, { to: 'b', kind: 'nudge' }), ev('message_sent', null, 6, { to: 'a', kind: 'message' })]
    const m = deriveTimeline([run('a'), run('b', { parent_run: 'a' })], events, NOW)
    expect(m.arrows).toEqual([
      { kind: 'message', from: 'a', to: 'b', at: T0 + 4000, msgKind: 'nudge' },
      { kind: 'message', from: ORCHESTRATOR_LANE, to: 'a', at: T0 + 6000, msgKind: 'message' },
    ])
  })

  it('creates escalated, nudge, check, review and join markers, and question and join spans', () => {
    const events = [
      ev('escalated', 'a', 1, { reason: 'stalled' }), ev('nudge_sent', 'a', 2),
      ev('question_asked', 'a', 3, { question: 'Which db?' }), ev('question_answered', 'a', 9),
      ev('check_result', 'a', 10, { ok: false, name: 'lint' }), ev('review_accepted', 'a', 11), ev('review_rejected', 'a', 12),
      ev('join_command_issued', 'a', 13), ev('user_joined', 'a', 14), ev('user_left', 'a', 20),
      ev('user_joined', null, 21),
    ]
    const m = deriveTimeline([run('a')], events, NOW)
    const l = lane(m, 'a')
    expect(l.markers.map((x) => [x.kind, x.ok])).toEqual([
      ['escalated', undefined], ['nudge', undefined], ['question', undefined], ['check', false], ['review', true], ['review', false], ['join', undefined],
    ])
    expect(l.markers[0].label).toBe('Escalated: stalled')
    expect(l.spans.map((s) => [s.kind, (s.end - s.start) / 1000, s.open])).toEqual([['question', 6, false], ['joined', 6, false]])
    expect(lane(m, ORCHESTRATOR_LANE).spans).toMatchObject([{ kind: 'joined', open: true }])
  })

  it('trusts state_changed over the tracked state and counts a warning', () => {
    const events = [ev('run_started', 'a', 0), ev('state_changed', 'a', 5, { from: 'waiting', to: 'running' })]
    const m = deriveTimeline([run('a')], events, NOW)
    expect(segs(lane(m, 'a')).map((s) => s[0])).toEqual(['starting', 'running'])
    expect(m.warnings).toBe(1)
  })

  it('sorts events by (ts, id) so out-of-order input gives the same model', () => {
    const events = [
      ev('run_queued', 'a', 0), ev('state_changed', 'a', 1, { from: 'queued', to: 'running' }),
      ev('state_changed', 'a', 9, { from: 'running', to: 'waiting' }),
    ]
    const shuffled = [events[2], events[0], events[1]]
    expect(deriveTimeline([run('a')], shuffled, NOW)).toEqual(deriveTimeline([run('a')], events, NOW))
  })

  it('orders lanes depth-first with children by created_at', () => {
    const runs = [
      run('child-late', { parent_run: 'root', created_at: iso(30) }),
      run('grand', { parent_run: 'child-early', created_at: iso(25) }),
      run('root', { created_at: iso(0) }),
      run('child-early', { parent_run: 'root', created_at: iso(10) }),
      run('other-root', { created_at: iso(5) }),
    ]
    const m = deriveTimeline(runs, [], NOW)
    expect(m.lanes.map((l) => [l.runId, l.depth])).toEqual([
      [ORCHESTRATOR_LANE, 0], ['root', 0], ['child-early', 1], ['grand', 2], ['child-late', 1], ['other-root', 0],
    ])
  })

  it('reports events of unknown runs without drawing them', () => {
    const m = deriveTimeline([run('a')], [ev('run_queued', 'ghost', 1)], NOW)
    expect(m.missingRuns).toEqual(['ghost'])
    expect(m.lanes).toHaveLength(2)
  })

  it('uses run created_at for tMin when no event is earlier', () => {
    const m = deriveTimeline([run('a', { created_at: iso(-10) })], [ev('run_queued', 'a', 0)], NOW)
    expect(m.tMin).toBe(T0 - 10_000)
  })
})

describe('applyEvent', () => {
  const sample = (): { runs: RunViewPublic[]; events: Event[] } => ({
    runs: [run('a'), run('b', { parent_run: 'a', created_at: iso(3) })],
    events: [
      ev('run_queued', 'a', 0), ev('attempt_started', 'a', 1, { account: 'x' }), ev('run_started', 'a', 1),
      ev('state_changed', 'a', 2, { from: 'starting', to: 'running' }),
      ev('tool_call_started', 'a', 3, { name: 'bash' }), ev('message_sent', 'a', 3, { to: 'b', kind: 'message' }),
      ev('run_queued', 'b', 4), ev('tool_call_finished', 'a', 5, { name: 'bash', ok: true }),
      ev('failover', 'a', 6, { from: 'x', to: 'y', reason: 'quota' }), ev('question_asked', 'b', 7), ev('question_answered', 'b', 8),
      ev('state_changed', 'a', 9, { from: 'running', to: 'waiting' }), ev('user_joined', null, 9),
      ev('run_finished', 'b', 12, { status: 'failed' }),
    ],
  })

  it('equals deriveTimeline for every prefix (fold equivalence)', () => {
    const { runs, events } = sample()
    let folded = deriveTimeline(runs, [], NOW)
    for (let i = 0; i < events.length; i++) {
      folded = applyEvent(folded, events[i], NOW)
      expect(folded).toEqual(deriveTimeline(runs, events.slice(0, i + 1), NOW))
    }
  })

  it('folds the large fixture to the same model as deriveTimeline', () => {
    const { session, events } = makeLargeSession({ events: 1500, runs: 12, seed: 4 })
    const now = Date.parse(session.ended_at!) + 1000
    let folded = deriveTimeline(session.runs, [], now)
    for (const e of events) folded = applyEvent(folded, e, now)
    expect(folded).toEqual(deriveTimeline(session.runs, events, now))
  })

  it('does not mutate the previous model and shares untouched lanes', () => {
    const { runs, events } = sample()
    const before = deriveTimeline(runs, events.slice(0, 8), NOW)
    const frozen = structuredClone(before)
    const next = applyEvent(before, ev('tool_call_started', 'b', 20, { name: 'grep' }), NOW)
    expect(before).toEqual(frozen)
    expect(lane(next, 'b').ticks).toHaveLength(1)
    expect(lane(next, 'a')).toBe(lane(before, 'a'))
    expect(lane(next, 'b')).not.toBe(lane(before, 'b'))
  })

  it('flags an event that sorts before lastKey for a full rebuild', () => {
    const { runs, events } = sample()
    const m = deriveTimeline(runs, events, NOW)
    const out = applyEvent(m, ev('nudge_sent', 'a', 1, {}, 'aaa'), NOW)
    expect(out.needsRebuild).toBe(true)
    expect(lane(out, 'a').markers).toEqual(lane(m, 'a').markers)
  })

  it('lists unknown runs in missingRuns', () => {
    const { runs } = sample()
    const out = applyEvent(deriveTimeline(runs, [], NOW), ev('run_queued', 'zzz', 1), NOW)
    expect(out.missingRuns).toEqual(['zzz'])
  })

  it('moves open segment ends when only now changes', () => {
    const { runs, events } = sample()
    const m = deriveTimeline(runs, events, NOW)
    const later = applyEvent(m, ev('assistant_message', 'a', 60), NOW + 7000)
    expect(lane(later, 'a').segments.at(-1)!.end).toBe(NOW + 7000)
    expect(later.tMax).toBe(NOW + 7000)
  })
})

describe('benchmark', () => {
  it('derives 10 000 events / 30 runs in under 200 ms', () => {
    const { session, events } = makeLargeSession({ events: 10000, runs: 30, transcriptRows: 0, seed: 1 })
    const now = Date.parse(session.ended_at!)
    deriveTimeline(session.runs, events, now) // warm up the JIT
    const t = performance.now()
    const m = deriveTimeline(session.runs, events, now)
    const ms = performance.now() - t
    console.info(`[perf] deriveTimeline 10000 events / 30 runs: ${ms.toFixed(1)} ms`)
    expect(m.lanes).toHaveLength(31)
    expect(m.warnings).toBe(0)
    expect(m.lanes.reduce((a, l) => a + l.ticks.length, 0)).toBeGreaterThan(3000)
    expect(ms).toBeLessThan(200)
  })

  it('applies 1 000 live events one by one in under 200 ms', () => {
    const { session, events } = makeLargeSession({ events: 10000, runs: 30, transcriptRows: 0, seed: 1 })
    const now = Date.parse(session.ended_at!)
    let m = deriveTimeline(session.runs, events.slice(0, 9000), now)
    const t = performance.now()
    for (const e of events.slice(9000)) m = applyEvent(m, e, now)
    const ms = performance.now() - t
    console.info(`[perf] applyEvent x1000 on a 9000-event model: ${ms.toFixed(1)} ms`)
    expect(m.needsRebuild).toBe(false)
    expect(ms).toBeLessThan(200)
  })
})
