import { Info } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { LEGEND } from './colors'
import { MarkerIcon, StateFill } from './glyphs'

/**
 * Swatches rely on the pattern definitions rendered once by `GanttCanvas`.
 * Every entry has text, so the legend is readable without colour.
 */
export function LegendContent({ compact = false }: { compact?: boolean }) {
  return (
    <div className={compact ? 'flex flex-wrap items-center gap-x-4 gap-y-1 text-xs' : 'grid gap-3 text-sm'}>
      <ul className={compact ? 'flex flex-wrap gap-x-3 gap-y-1' : 'grid grid-cols-2 gap-x-4 gap-y-1.5'} aria-label="Run states">
        {LEGEND.states.map((s) => (
          <li key={s.state} className="flex items-center gap-1.5">
            <svg width={28} height={14} aria-hidden focusable="false">
              <StateFill state={s.state} x={0} y={0} w={28} h={14} />
            </svg>
            <span>{s.label}</span>
            {!compact && <span className="text-xs text-muted-foreground">{s.patternLabel}</span>}
          </li>
        ))}
      </ul>
      <ul className={compact ? 'flex flex-wrap gap-x-3 gap-y-1' : 'grid grid-cols-2 gap-x-4 gap-y-1.5'} aria-label="Markers">
        {LEGEND.markers.map((m) => (
          <li key={m.kind} className="flex items-center gap-1.5">
            <MarkerIcon kind={m.kind} />
            <span>{m.label}</span>
            {!compact && <span className="text-xs text-muted-foreground">{m.description}</span>}
          </li>
        ))}
      </ul>
    </div>
  )
}

/** Popover trigger; on `xl` the page also pins `LegendContent` in the header. */
export function Legend() {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button type="button" variant="outline" size="sm" className="xl:hidden">
          <Info aria-hidden />
          Legend
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[26rem] max-w-[90vw]">
        <LegendContent />
      </PopoverContent>
    </Popover>
  )
}
