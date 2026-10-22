import { describe, expect, it } from 'vitest'
import { makeLargeSession, prng } from './large'

describe('makeLargeSession', () => {
  it('is deterministic for a seed and differs across seeds', () => {
    const o = { events: 800, runs: 8, transcriptRows: 200, seed: 7 }
    const a = makeLargeSession(o)
    const b = makeLargeSession(o)
    expect(JSON.stringify(a)).toBe(JSON.stringify(b))
    expect(JSON.stringify(makeLargeSession({ ...o, seed: 8 }))).not.toBe(JSON.stringify(a))
  })

  it('honours the requested sizes', () => {
    const l = makeLargeSession({ events: 10000, runs: 30, transcriptRows: 10000, seed: 1 })
    expect(l.events).toHaveLength(10000)
    expect(l.session.runs).toHaveLength(30)
    expect(l.session.run_count).toBe(30)
    expect(l.transcript).toHaveLength(10000)
  })

  it('orders events by (ts, id) with unique ids', () => {
    const { events } = makeLargeSession({ events: 3000, runs: 12, seed: 3 })
    const ids = new Set(events.map((e) => e.id))
    expect(ids.size).toBe(events.length)
    for (let i = 1; i < events.length; i++) {
      const a = events[i - 1]
      const b = events[i]
      expect(a.ts < b.ts || (a.ts === b.ts && a.id < b.id)).toBe(true)
    }
  })

  it('references only known runs and builds a parent tree', () => {
    const { events, session } = makeLargeSession({ events: 2000, runs: 20, seed: 2 })
    const ids = new Set(session.runs.map((r) => r.run_id))
    expect(events.every((e) => e.run_id === null || ids.has(e.run_id))).toBe(true)
    expect(session.runs.filter((r) => !r.parent_run)).toHaveLength(1)
    expect(session.runs.every((r) => !r.parent_run || ids.has(r.parent_run))).toBe(true)
  })

  it('has strictly increasing transcript byte offsets', () => {
    const { transcript } = makeLargeSession({ events: 100, runs: 3, transcriptRows: 300, seed: 1 })
    for (let i = 1; i < transcript.length; i++) expect(transcript[i].offset).toBeGreaterThan(transcript[i - 1].offset)
  })
})

describe('prng', () => {
  it('stays in [0, 1)', () => {
    const r = prng(5)
    for (let i = 0; i < 1000; i++) {
      const v = r()
      expect(v).toBeGreaterThanOrEqual(0)
      expect(v).toBeLessThan(1)
    }
  })
})
