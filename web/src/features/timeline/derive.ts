import type { Event, RunViewPublic } from '@/api/types'
import { applyCore, compareKeys, eventMs, refreshNow } from './applyEvent'
import { ORCHESTRATOR_LANE, type RunLane, type TimelineModel } from './model'

function newLane(runId: string, depth: number, run?: RunViewPublic): RunLane {
  return {
    runId, run, depth,
    segments: [], attempts: [], ticks: [], markers: [], spans: [],
    terminal: false, eventCount: 0, cur: null, pendingTools: {}, openQuestions: [], openJoins: [],
  }
}

const createdMs = (r: RunViewPublic) => (r.created_at ? Date.parse(r.created_at) : Number.POSITIVE_INFINITY)

/** Lane order: orchestrator first, then depth-first over `parent_run`, children by `created_at`. */
export function laneOrder(runs: RunViewPublic[]): { run: RunViewPublic; depth: number }[] {
  const byId = new Map(runs.map((r) => [r.run_id, r]))
  const children = new Map<string, RunViewPublic[]>()
  const roots: RunViewPublic[] = []
  for (const r of runs) {
    if (r.parent_run && byId.has(r.parent_run) && r.parent_run !== r.run_id) {
      const list = children.get(r.parent_run) ?? []
      list.push(r)
      children.set(r.parent_run, list)
    } else roots.push(r)
  }
  const cmp = (a: RunViewPublic, b: RunViewPublic) => createdMs(a) - createdMs(b) || (a.run_id < b.run_id ? -1 : 1)
  const out: { run: RunViewPublic; depth: number }[] = []
  const seen = new Set<string>()
  const walk = (r: RunViewPublic, depth: number) => {
    if (seen.has(r.run_id)) return
    seen.add(r.run_id)
    out.push({ run: r, depth })
    for (const c of (children.get(r.run_id) ?? []).sort(cmp)) walk(c, depth + 1)
  }
  for (const r of roots.sort(cmp)) walk(r, 0)
  for (const r of runs) walk(r, 0) // runs in a parent cycle: still show them
  return out
}

/** An empty model with a lane per run (and the orchestrator lane). */
export function emptyModel(runs: RunViewPublic[], now: number): TimelineModel {
  const lanes: RunLane[] = [newLane(ORCHESTRATOR_LANE, 0)]
  for (const { run, depth } of laneOrder(runs)) lanes.push(newLane(run.run_id, depth, run))
  const index: Record<string, number> = {}
  lanes.forEach((l, i) => (index[l.runId] = i))
  return {
    lanes, index, arrows: [], tMin: now, tMax: now + 1000, now, active: false,
    lastKey: null, firstTs: null, warnings: 0, missingRuns: [], needsRebuild: false,
  }
}

/**
 * Pure derivation (design 4.2). `events` need not be sorted; they are sorted once by
 * `(ts, id)`. No clock is read: callers pass `now`. Equal to folding `applyEvent`.
 */
export function deriveTimeline(runs: RunViewPublic[], events: Event[], now: number): TimelineModel {
  const model = emptyModel(runs, now)
  const keyed: { ev: Event; ms: number }[] = []
  for (const ev of events) {
    const ms = eventMs(ev)
    if (Number.isFinite(ms)) keyed.push({ ev, ms })
  }
  keyed.sort((a, b) => compareKeys({ ts: a.ms, id: a.ev.id }, { ts: b.ms, id: b.ev.id }))
  const missing = new Set<string>()
  for (const { ev, ms } of keyed) {
    const idx = model.index[ev.run_id ?? ORCHESTRATOR_LANE]
    model.lastKey = { ts: ms, id: ev.id }
    model.firstTs ??= ms
    if (idx === undefined) {
      missing.add(ev.run_id as string)
      continue
    }
    applyCore(model, model.lanes[idx], ev, ms)
  }
  model.missingRuns = [...missing]
  return refreshNow(model, now)
}
