import { describe, expect, it } from 'vitest'
import type { Tick } from './model'
import {
  clampViewport, clusterTicks, fitViewport, lowerBound, MIN_SPAN_MS, panBy, sliderToSpan, span, spanToSlider,
  timeToX, visibleRange, visibleSlice, withSpan, xToTime, zoomAt,
} from './viewport'

const B = { tMin: 0, tMax: 600_000 } // 10 minutes

describe('viewport math', () => {
  it('fits the whole session with a small pad', () => {
    const v = fitViewport(B)
    expect(v.t0).toBeLessThan(0)
    expect(v.t1).toBeGreaterThan(600_000)
  })

  it('zooms around the anchor, keeping its screen fraction', () => {
    const v = { t0: 0, t1: 600_000 }
    const z = zoomAt(v, 0.5, 150_000, B)
    expect(span(z)).toBe(300_000)
    expect((150_000 - z.t0) / span(z)).toBeCloseTo(0.25, 6)
  })

  it('enforces the 1 s minimum and 2x session maximum', () => {
    const v = { t0: 0, t1: 600_000 }
    expect(span(zoomAt(v, 1e-9, 1000, B))).toBeCloseTo(MIN_SPAN_MS, 6)
    expect(span(zoomAt(v, 1e9, 1000, B))).toBeCloseTo(1_200_000, 6)
  })

  it('pans and keeps the viewport centre inside the session', () => {
    const v = { t0: 100_000, t1: 200_000 }
    expect(panBy(v, 50_000, B)).toEqual({ t0: 150_000, t1: 250_000 })
    const far = panBy(v, 1e9, B)
    expect(far.t0 + span(far) / 2).toBeLessThanOrEqual(B.tMax)
    const farLeft = panBy(v, -1e9, B)
    expect(farLeft.t0 + span(farLeft) / 2).toBeGreaterThanOrEqual(B.tMin)
    expect(span(far)).toBe(100_000)
  })

  it('clamps an out-of-range viewport', () => {
    const c = clampViewport({ t0: 5e9, t1: 5e9 + 10 }, B)
    expect(span(c)).toBe(MIN_SPAN_MS)
    expect(c.t0).toBeLessThanOrEqual(B.tMax)
  })

  it('maps time and x both ways', () => {
    const v = { t0: 1000, t1: 3000 }
    expect(timeToX(2000, v, 400)).toBe(200)
    expect(xToTime(200, v, 400)).toBe(2000)
  })

  it('round-trips the zoom slider and keeps the centre when setting the span', () => {
    expect(spanToSlider(sliderToSpan(0, B), B)).toBe(0)
    expect(spanToSlider(sliderToSpan(100, B), B)).toBe(100)
    expect(spanToSlider(sliderToSpan(37, B), B)).toBe(37)
    const v = withSpan({ t0: 100_000, t1: 200_000 }, 40_000, B)
    expect(v).toEqual({ t0: 130_000, t1: 170_000 })
  })
})

describe('culling', () => {
  it('lowerBound finds the first index at or above a key', () => {
    const a = [1, 3, 3, 7]
    expect(lowerBound(a, 3, (x) => x)).toBe(1)
    expect(lowerBound(a, 8, (x) => x)).toBe(4)
    expect(lowerBound(a, 0, (x) => x)).toBe(0)
  })
  it('visibleSlice returns segments touching the window, including one that starts earlier', () => {
    const segs = Array.from({ length: 100 }, (_, i) => ({ s: i * 10, e: i * 10 + 10 }))
    const long = segs.map((x) => (x.s === 150 ? { s: 150, e: 500 } : x))
    const out = visibleSlice(segs, 200, 230, (x) => x.s, (x) => x.e)
    expect(out.map((x) => x.s)).toEqual([190, 200, 210, 220, 230])
    expect(visibleSlice(long, 200, 230, (x) => x.s, (x) => x.e)[0]).toEqual({ s: 150, e: 500 })
    expect(visibleSlice(segs, 5000, 6000, (x) => x.s, (x) => x.e)).toEqual([])
  })
})

describe('visibleRange', () => {
  it('includes an item that starts exactly at t1', () => {
    const items = [{ s: 0 }, { s: 10 }, { s: 20 }, { s: 30 }]
    const [from, to] = visibleRange(items, 10, 20, (x) => x.s)
    expect(items.slice(from, to).map((x) => x.s)).toEqual([0, 10, 20])
    expect(visibleRange([], 0, 1, (x: { s: number }) => x.s)).toEqual([0, 0])
  })
})

describe('clusterTicks', () => {
  const mk = (at: number, ok: boolean | null = true): Tick => ({ kind: 'tool', runId: 'a', at, name: 'bash', ok })
  const v = { t0: 0, t1: 1000 }

  it('merges ticks in the same 3 px bucket and splits when zoomed in', () => {
    const ticks = [mk(100), mk(101), mk(102, false), mk(500)]
    const wide = clusterTicks(ticks, v, 1000) // 1 px per ms
    // 100,101,102 fall into buckets floor(100/3)=33, 33, 34 at 1px/ms
    expect(wide.reduce((a, c) => a + c.count, 0)).toBe(4)
    const coarse = clusterTicks(ticks, v, 100) // 0.1 px per ms: first three share a bucket
    expect(coarse[0]).toMatchObject({ count: 3, failed: true })
    expect(coarse).toHaveLength(2)
    const fine = clusterTicks(ticks, { t0: 99, t1: 104 }, 1000)
    expect(fine.map((c) => c.count)).toEqual([1, 1, 1])
  })

  it('culls ticks outside the window and flags pending ones', () => {
    const ticks = [mk(10), mk(500, null), mk(990)]
    const out = clusterTicks(ticks, { t0: 400, t1: 600 }, 400)
    expect(out).toHaveLength(1)
    expect(out[0].pending).toBe(true)
  })

  it('stays small for dense ticks: at most width/3 clusters', () => {
    const ticks = Array.from({ length: 10_000 }, (_, i) => mk(i * 0.1))
    const out = clusterTicks(ticks, v, 900)
    expect(out.length).toBeLessThanOrEqual(900 / 3 + 3)
    expect(out.reduce((a, c) => a + c.count, 0)).toBeGreaterThanOrEqual(10_000 - 1)
  })
})
