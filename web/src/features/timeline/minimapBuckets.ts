import type { StateSegment } from './model'
import { timeToX } from './viewport'

export interface MiniRect {
  x: number
  w: number
  segment: StateSegment
}

/** At most one rect per pixel column: the first segment that starts in a column wins; runs are at least 1 px wide. */
export function bucketSegments(segments: readonly StateSegment[], tMin: number, tMax: number, width: number): MiniRect[] {
  const full = { t0: tMin, t1: tMax }
  const out: MiniRect[] = []
  let lastCol = -1
  for (const s of segments) {
    const x0 = timeToX(s.start, full, width)
    const col = Math.floor(x0)
    if (col === lastCol) continue
    lastCol = col
    const x1 = timeToX(s.end, full, width)
    out.push({ x: x0, w: Math.max(x1 - x0, 1), segment: s })
  }
  return out
}
