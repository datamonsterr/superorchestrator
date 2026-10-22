import { useMemo, useState } from 'react'
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from 'recharts'
import { ChartContainer, ChartLegend, ChartLegendContent, ChartTooltip, ChartTooltipContent, type ChartConfig } from '@/components/ui/chart'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { EmptyState } from '@/components/common/EmptyState'
import { BarChart3 } from 'lucide-react'
import { formatMoney, formatTokens } from '@/lib/format'
import type { DailyUsage } from '@/api/types'
import { chartRows, type Mode } from './usage'

const PALETTE = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)']

export function UsageChart({ daily }: { daily: DailyUsage[] }) {
  const [mode, setMode] = useState<Mode>('cost')
  const { rows, series } = useMemo(() => chartRows(daily, mode), [daily, mode])
  const config = useMemo<ChartConfig>(
    () => Object.fromEntries(series.map((s, i) => [s, { label: s, color: PALETTE[i % PALETTE.length] }])),
    [series],
  )
  const fmt = mode === 'cost' ? formatMoney : formatTokens
  const total = daily.reduce((n, d) => n + (mode === 'cost' ? d.cost : d.input_tokens + d.output_tokens), 0)
  if (daily.length === 0) return <EmptyState icon={BarChart3} title="No usage yet" description="Usage appears here once runs report cost." />
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-muted-foreground">
          {daily.length} days, total <span className="font-mono text-foreground">{fmt(total)}</span>
        </p>
        <Tabs value={mode} onValueChange={(v) => setMode(v as Mode)}>
          <TabsList>
            <TabsTrigger value="cost">Cost</TabsTrigger>
            <TabsTrigger value="tokens">Tokens</TabsTrigger>
          </TabsList>
        </Tabs>
      </div>
      <ChartContainer config={config} className="h-56 w-full" role="img" aria-label={`${mode === 'cost' ? 'Cost' : 'Tokens'} per day`}>
        <BarChart data={rows} margin={{ left: 0, right: 0 }}>
          <CartesianGrid vertical={false} />
          <XAxis dataKey="day" tickLine={false} axisLine={false} tickFormatter={(d: string) => d.slice(5)} minTickGap={24} />
          <YAxis tickLine={false} axisLine={false} width={48} tickFormatter={(v: number) => fmt(v)} />
          <ChartTooltip content={<ChartTooltipContent />} />
          <ChartLegend content={<ChartLegendContent />} />
          {series.map((s) => (
            <Bar key={s} dataKey={s} stackId="a" fill={`var(--color-${s})`} isAnimationActive={false} />
          ))}
        </BarChart>
      </ChartContainer>
    </div>
  )
}
