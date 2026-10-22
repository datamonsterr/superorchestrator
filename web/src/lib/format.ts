export function formatMoney(value: number): string {
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 2,
    maximumFractionDigits: value !== 0 && Math.abs(value) < 0.1 ? 4 : 2,
  }).format(value)
}

export function formatTokens(n: number): string {
  if (Math.abs(n) < 1000) return String(n)
  const units: [number, string][] = [
    [1e9, 'B'],
    [1e6, 'M'],
    [1e3, 'k'],
  ]
  for (const [size, suffix] of units) {
    if (Math.abs(n) >= size) {
      const v = n / size
      return `${v >= 100 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, '')}${suffix}`
    }
  }
  return String(n)
}

export function formatPercent(pct: number | null | undefined): string {
  if (pct == null || Number.isNaN(pct)) return 'unknown'
  return `${Math.round(pct)}%`
}

/** 0 -> "0s", 65_000 -> "1m 5s", 3_700_000 -> "1h 1m". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0s'
  const total = Math.floor(ms / 1000)
  if (total < 60) return `${total}s`
  const m = Math.floor(total / 60)
  if (m < 60) return `${m}m ${total % 60}s`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ${m % 60}m`
  return `${Math.floor(h / 24)}d ${h % 24}h`
}

const RTF = new Intl.RelativeTimeFormat('en', { numeric: 'auto' })
const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['second', 60],
  ['minute', 60],
  ['hour', 24],
  ['day', 30],
  ['month', 12],
  ['year', Number.POSITIVE_INFINITY],
]

/** Relative to `now` (epoch ms), e.g. "5 minutes ago", "in 2 hours". */
export function formatRelative(ts: string | number | Date, now: number): string {
  const t = new Date(ts).getTime()
  if (Number.isNaN(t)) return 'unknown'
  let delta = Math.round((t - now) / 1000)
  if (Math.abs(delta) < 5) return 'just now'
  for (const [unit, size] of STEPS) {
    if (Math.abs(delta) < size) return RTF.format(delta, unit)
    delta = Math.round(delta / size)
  }
  return RTF.format(delta, 'year')
}

export function stringifyJson(value: unknown): string {
  if (value === undefined) return 'undefined'
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return '[unserialisable value]'
  }
}
