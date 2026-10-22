export type CapLevel = 'ok' | 'warn' | 'full'
/** Amber at 80 percent, red at 100 percent (design 1.1). */
export function capLevel(active: number, cap: number): CapLevel {
  if (cap <= 0) return 'ok'
  const r = active / cap
  return r >= 1 ? 'full' : r >= 0.8 ? 'warn' : 'ok'
}
