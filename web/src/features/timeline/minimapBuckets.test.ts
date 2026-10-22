import { describe, expect, it } from 'vitest'
import type { StateSegment } from './model'
import { bucketSegments } from './minimapBuckets'

const seg = (start: number, end: number): StateSegment => ({ kind: 'state', runId: 'a', state: 'running', start, end, open: false })

describe('bucketSegments', () => {
  it('keeps at most one rect per pixel column', () => {
    const segs = Array.from({ length: 5000 }, (_, i) => seg(i, i + 1)) // 5000 segments over 100 px
    const rects = bucketSegments(segs, 0, 5000, 100)
    expect(rects.length).toBeLessThanOrEqual(100)
    expect(rects.length).toBeGreaterThan(90)
    expect(new Set(rects.map((r) => Math.floor(r.x))).size).toBe(rects.length)
  })
  it('gives every rect at least one pixel and keeps sparse segments as they are', () => {
    const rects = bucketSegments([seg(0, 10), seg(500, 500)], 0, 1000, 200)
    expect(rects).toHaveLength(2)
    expect(rects[1].w).toBe(1)
    expect(rects[0].x).toBe(0)
  })
})
