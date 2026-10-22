import type { DailyUsage } from '@/api/types'

export type Mode = 'cost' | 'tokens'

/** Rows for recharts: cost is stacked per account, tokens per direction. Pure, so it is unit tested. */
export function chartRows(daily: DailyUsage[], mode: Mode): { rows: Record<string, number | string>[]; series: string[] } {
  if (mode === 'tokens') {
    return {
      series: ['input', 'output'],
      rows: daily.map((d) => ({ day: d.day, input: d.input_tokens, output: d.output_tokens })),
    }
  }
  const series = [...new Set(daily.flatMap((d) => Object.keys(d.by_account)))].sort()
  return { series, rows: daily.map((d) => ({ day: d.day, ...Object.fromEntries(series.map((s) => [s, d.by_account[s] ?? 0])) })) }
}

