/** Pure viewport math for the Gantt: zoom, pan, fit, scales, culling and tick clustering. */
import type { Tick } from './model'

export interface Viewport {
  t0: number
  t1: number
}
export interface Bounds {
  tMin: number
  tMax: number
}

export const MIN_SPAN_MS = 1000
export const span = (v: Viewport) => v.t1 - v.t0

export function spanLimits({ tMin, tMax }: Bounds): { min: number; max: number } {
  return { min: MIN_SPAN_MS, max: Math.max(2 * (tMax - tMin), MIN_SPAN_MS * 2) }
}

/** Keeps the span within limits and the viewport centre inside the session. */
export function clampViewport(v: Viewport, b: Bounds): Viewport {
  const lim = spanLimits(b)
  const s = Math.min(Math.max(span(v), lim.min), lim.max)
  const t0 = Math.min(Math.max(v.t0, b.tMin - s / 2), b.tMax - s / 2)
  return { t0, t1: t0 + s }
}

/** `Fit`: the whole session. */
export const fitViewport = (b: Bounds): Viewport => {
  const pad = Math.max((b.tMax - b.tMin) * 0.01, 0)
  return { t0: b.tMin - pad, t1: b.tMax + pad }
}

/** Scales the span by `factor` (below 1 zooms in) keeping `anchor` at the same screen fraction. */
export function zoomAt(v: Viewport, factor: number, anchor: number, b: Bounds): Viewport {
  const lim = spanLimits(b)
  const s = Math.min(Math.max(span(v) * factor, lim.min), lim.max)
  const frac = span(v) === 0 ? 0.5 : (anchor - v.t0) / span(v)
  const t0 = anchor - frac * s
  return clampViewport({ t0, t1: t0 + s }, b)
}

export const panBy = (v: Viewport, dt: number, b: Bounds): Viewport => clampViewport({ t0: v.t0 + dt, t1: v.t1 + dt }, b)

export const timeToX = (t: number, v: Viewport, width: number): number => ((t - v.t0) / span(v)) * width
export const xToTime = (x: number, v: Viewport, width: number): number => v.t0 + (x / width) * span(v)

/** Slider value 0 (fully zoomed out) to 100 (fully zoomed in), logarithmic in the span. */
export function spanToSlider(s: number, b: Bounds): number {
  const { min, max } = spanLimits(b)
  const clamped = Math.min(Math.max(s, min), max)
  return Math.round((1 - Math.log(clamped / min) / Math.log(max / min)) * 100)
}
export function sliderToSpan(value: number, b: Bounds): number {
  const { min, max } = spanLimits(b)
  return min * Math.pow(max / min, 1 - value / 100)
}

/** Sets the span, keeping the viewport centre. */
export function withSpan(v: Viewport, s: number, b: Bounds): Viewport {
  const c = v.t0 + span(v) / 2
  return clampViewport({ t0: c - s / 2, t1: c + s / 2 }, b)
}

/** First index in sorted `arr` whose `key(item) >= target`. */
export function lowerBound<T>(arr: readonly T[], target: number, key: (item: T) => number): number {
  let lo = 0
  let hi = arr.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (key(arr[mid]) < target) lo = mid + 1
    else hi = mid
  }
  return lo
}

/**
 * Items of a time-sorted, non-overlapping-or-nearly list that touch `[t0, t1]`.
 * Items must be sorted by `start`; `end` may be non-monotonic only by overlap, so a
 * short look-behind (`back`) is applied to catch long items starting earlier.
 */
export function visibleSlice<T>(arr: readonly T[], t0: number, t1: number, start: (i: T) => number, end: (i: T) => number, back = 8): T[] {
  let i = Math.max(0, lowerBound(arr, t0, start) - back)
  const out: T[] = []
  for (; i < arr.length; i++) {
    const it = arr[i]
    if (start(it) > t1) break
    if (end(it) >= t0) out.push(it)
  }
  return out
}

export interface TickCluster {
  /** Bucket index: `floor(x / bucketPx)`. */
  bucket: number
  x: number
  count: number
  first: Tick
  /** Index of `first` in the lane's tick list. */
  index: number
  /** Any tick in the bucket failed / is still pending. */
  failed: boolean
  pending: boolean
}

/** Clusters ticks in `[t0 - pad, t1 + pad]` into pixel buckets (design 4.5). Ticks must be sorted by time. */
export function clusterTicks(ticks: readonly Tick[], v: Viewport, width: number, bucketPx = 3, padPx = 6): TickCluster[] {
  const padT = (padPx / width) * span(v)
  const lo = lowerBound(ticks, v.t0 - padT, (t) => t.at)
  const out: TickCluster[] = []
  let cur: TickCluster | null = null
  for (let i = lo; i < ticks.length; i++) {
    const t = ticks[i]
    if (t.at > v.t1 + padT) break
    const x = timeToX(t.at, v, width)
    const bucket = Math.floor(x / bucketPx)
    if (cur && cur.bucket === bucket) {
      cur.count++
      cur.failed ||= t.ok === false
      cur.pending ||= t.ok === null
    } else {
      cur = { bucket, x, count: 1, first: t, index: i, failed: t.ok === false, pending: t.ok === null }
      out.push(cur)
    }
  }
  return out
}

/**
 * Index range `[from, to)` of a list sorted by `start` that may touch `[t0, t1]`
 * (one item of look-behind for a segment that started before `t0`). Callers still test `end`.
 */
export function visibleRange<T>(arr: readonly T[], t0: number, t1: number, start: (i: T) => number): [number, number] {
  const from = Math.max(0, lowerBound(arr, t0, start) - 1)
  const to = lowerBound(arr, t1 + Number.EPSILON, start)
  return [from, Math.max(from, to)]
}
