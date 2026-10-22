import type {
  Event,
  EventType,
  Harness,
  RunState,
  RunViewPublic,
  SessionDetail,
  TranscriptLine,
} from '@/api/types'

export interface LargeSessionOptions {
  events?: number
  runs?: number
  transcriptRows?: number
  seed?: number
}
export interface LargeSession {
  session: SessionDetail
  /** Sorted by (ts, id); ids are zero padded so string order equals sequence order. */
  events: Event[]
  /** Transcript for every run of the session (byte offsets are cumulative). */
  transcript: TranscriptLine[]
}

/** mulberry32: small, fast, deterministic. */
export function prng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

export const LARGE_SESSION_ID = 'sess-large'
const BASE_MS = Date.UTC(2026, 0, 1, 12, 0, 0)
const HARNESSES: Harness[] = ['codex', 'agy', 'opencode', 'claude']
const TOOLS = ['read_file', 'write_file', 'bash', 'grep', 'edit', 'web_search']
const ROLES: TranscriptLine['role'][] = ['user', 'assistant', 'tool', 'system']
const pad = (n: number, w: number) => String(n).padStart(w, '0')

/**
 * Deterministic big session for perf work: no clocks and no Math.random, so the
 * same options always yield the same output. Used by MSW (`sess-large`) and by
 * `pnpm dev:mock` with `?large=1`.
 */
export function makeLargeSession({
  events: eventCount = 10000,
  runs: runCount = 30,
  transcriptRows = 10000,
  seed = 1,
}: LargeSessionOptions = {}): LargeSession {
  const rnd = prng(seed)
  const int = (lo: number, hi: number) => lo + Math.floor(rnd() * (hi - lo + 1))
  const pick = <T,>(xs: T[]) => xs[int(0, xs.length - 1)]
  const iso = (ms: number) => new Date(ms).toISOString()
  const sessionMs = 3 * 60 * 60 * 1000

  type Draft = { ms: number; order: number; run_id: string | null; type: EventType; data: unknown }
  const drafts: Draft[] = []
  let order = 0
  const add = (ms: number, run_id: string | null, type: EventType, data: unknown = {}) =>
    drafts.push({ ms, order: order++, run_id, type, data })

  const runs: RunViewPublic[] = []
  const windows: { id: string; start: number; end: number }[] = []
  let cost = 0
  for (let i = 0; i < runCount; i++) {
    const id = `run-${pad(i, 3)}`
    const parentIdx = i === 0 ? -1 : Math.floor((i - 1) / 3)
    const parent = parentIdx >= 0 ? runs[parentIdx] : undefined
    const pw = parentIdx >= 0 ? windows[parentIdx] : undefined
    const start = pw ? pw.start + int(1, 60) * 1000 + (i % 3) * 20000 : BASE_MS
    const end = Math.min(BASE_MS + sessionMs, start + int(2, 40) * 60 * 1000)
    const harness = pick(HARNESSES)
    const account = `${harness}-acc${int(1, 2)}`
    const usage = { input_tokens: int(1e4, 5e5), output_tokens: int(1e3, 5e4), cost: Math.round(rnd() * 400) / 100 }
    cost += usage.cost
    const failed = rnd() < 0.1
    const finalState: RunState = failed ? 'failed' : 'succeeded'
    windows.push({ id, start, end })
    runs.push({
      run_id: id,
      state: finalState,
      harness,
      account,
      model: `${harness}-model`,
      recent_events: [],
      usage,
      name: `task-${pad(i, 3)}`,
      depth: parent ? parent.depth + 1 : 0,
      parent_run: parent?.run_id,
      created_at: iso(start),
      finished_at: iso(end),
      cwd: '/work/repo',
      ...(failed ? { error_kind: pick(['quota', 'rate_limit', 'other'] as const) } : {}),
    })

    // Lifecycle: queued -> starting -> running (-> waiting/stalled)* -> terminal.
    add(start, id, 'run_queued', { brief: `Do task ${i}`, isolation: 'worktree' })
    add(start + 500, id, 'attempt_started', { account })
    add(start + 1000, id, 'state_changed', { from: 'queued', to: 'starting' })
    add(start + 2000, id, 'run_started', {})
    const span = end - start
    const changes = int(1, 4)
    const marks = Array.from({ length: changes }, () => start + 3000 + Math.floor(rnd() * (span - 6000))).sort((a, b) => a - b)
    let cur: RunState = 'running'
    add(start + 2500, id, 'state_changed', { from: 'starting', to: 'running' })
    for (const ms of marks) {
      const next: RunState = cur === 'running' ? (rnd() < 0.5 ? 'waiting' : 'stalled') : 'running'
      add(ms, id, 'state_changed', { from: cur, to: next })
      if (next === 'stalled') add(ms, id, 'stall_detected', { idle_ms: 60000 })
      cur = next
    }
    if (cur !== 'running') add(end - 2500, id, 'state_changed', { from: cur, to: 'running' })
    if (rnd() < 0.2) {
      add(start + Math.floor(span / 2), id, 'failover', { from: account, to: `${harness}-acc3`, reason: 'quota' })
    }
    add(end - 2000, id, 'usage', usage)
    add(end - 1000, id, 'state_changed', { from: 'running', to: finalState })
    add(end, id, 'run_finished', { status: finalState })
    if (parent && pw) add(start + 100, parent.run_id, 'message_sent', { to: id, kind: 'message' })
  }

  // Fill with tool-call pairs inside run windows.
  while (drafts.length < eventCount) {
    const w = windows[int(0, windows.length - 1)]
    const ms = w.start + 3000 + Math.floor(rnd() * Math.max(1, w.end - w.start - 6000))
    const name = pick(TOOLS)
    add(ms, w.id, 'tool_call_started', { name })
    if (drafts.length < eventCount) add(ms + int(50, 4000), w.id, 'tool_call_finished', { name, ok: rnd() > 0.05 })
  }

  drafts.sort((a, b) => a.ms - b.ms || a.order - b.order)
  const events: Event[] = drafts.slice(0, eventCount).map((d, i) => ({
    id: `ev-${pad(i + 1, 7)}`,
    session_id: LARGE_SESSION_ID,
    run_id: d.run_id,
    ts: iso(d.ms),
    type: d.type,
    data: d.data,
  }))

  // Transcript rows over the runs, cumulative byte offsets.
  const transcript: TranscriptLine[] = []
  let offset = 0
  for (let i = 0; i < transcriptRows; i++) {
    const role = ROLES[i % ROLES.length]
    const text =
      role === 'tool'
        ? `${pick(TOOLS)} ok ${pad(i, 6)}`
        : `${role} line ${i}: ${'lorem ipsum dolor sit amet '.repeat(int(1, 6)).trim()}`
    transcript.push({
      offset,
      role,
      text,
      ts: iso(BASE_MS + i * 1000),
      ...(role === 'tool' ? { tool: { name: pick(TOOLS), ok: rnd() > 0.05 } } : {}),
    })
    offset += text.length + 1
  }

  const session: SessionDetail = {
    id: LARGE_SESSION_ID,
    kind: 'orchestrator',
    client: 'claude-code',
    cwd: '/work/repo',
    started_at: iso(BASE_MS),
    ended_at: iso(BASE_MS + sessionMs),
    status: 'done',
    run_count: runCount,
    cost: Math.round(cost * 100) / 100,
    runs,
  }
  return { session, events, transcript }
}
