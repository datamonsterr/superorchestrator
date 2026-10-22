/** Epoch-millisecond helpers and axis tick selection for the timeline. */

export const SECOND = 1000
export const MINUTE = 60 * SECOND
export const HOUR = 60 * MINUTE

/** Candidate axis steps: 1 s, 5 s, 15 s, 1 m, 5 m, 15 m, 1 h (design 4.4), plus the in-betweens that keep 6 to 12 labels at every zoom. */
export const NICE_STEPS_MS = [
  SECOND, 2 * SECOND, 5 * SECOND, 10 * SECOND, 15 * SECOND, 30 * SECOND,
  MINUTE, 2 * MINUTE, 5 * MINUTE, 10 * MINUTE, 15 * MINUTE, 30 * MINUTE,
  HOUR, 2 * HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, 24 * HOUR,
]

/** Smallest nice step that yields at most `maxLabels` labels over `spanMs`. */
export function niceStep(spanMs: number, maxLabels = 12): number {
  for (const s of NICE_STEPS_MS) if (spanMs / s <= maxLabels) return s
  return NICE_STEPS_MS[NICE_STEPS_MS.length - 1]
}

export interface AxisTick {
  t: number
  /** Multiple of the next larger step: drawn taller and labelled with the date when it is midnight. */
  major: boolean
}

/**
 * Ticks at multiples of the nice step inside `[t0, t1]`. `offsetMs` is the local UTC offset
 * (default: that of `t0` in the runtime time zone), so hour steps land on local hours.
 */
export function axisTicks(t0: number, t1: number, maxLabels = 12, offsetMs = -new Date(t0).getTimezoneOffset() * MINUTE): AxisTick[] {
  if (!(t1 > t0)) return []
  const step = niceStep(t1 - t0, maxLabels)
  const bigger = NICE_STEPS_MS[Math.min(NICE_STEPS_MS.indexOf(step) + 1, NICE_STEPS_MS.length - 1)]
  const out: AxisTick[] = []
  for (let k = Math.ceil((t0 + offsetMs) / step); ; k++) {
    const t = k * step - offsetMs
    if (t > t1) break
    out.push({ t, major: (k * step) % bigger === 0 })
  }
  return out
}

const p2 = (n: number) => String(n).padStart(2, '0')

/** Local `HH:MM` or `HH:MM:SS`. */
export function formatClock(ms: number, seconds = false): string {
  const d = new Date(ms)
  const base = `${p2(d.getHours())}:${p2(d.getMinutes())}`
  return seconds ? `${base}:${p2(d.getSeconds())}` : base
}

/** Axis label: seconds only when the step is below a minute. */
export function formatAxisLabel(ms: number, stepMs: number): string {
  return formatClock(ms, stepMs < MINUTE)
}
