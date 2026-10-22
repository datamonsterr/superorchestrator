import { useEffect, useRef, useState, type RefObject } from 'react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { describeTip } from './layout'
import type { TimelineModel } from './model'

interface Tip {
  x: number
  y: number
  lines: string[]
}

/**
 * One tooltip for the whole plot (a Radix Tooltip per bar would be thousands of instances). It reads
 * `data-tip` off the hovered element and anchors a 1 px trigger at the pointer.
 */
export function PlotTooltip({ containerRef, model }: { containerRef: RefObject<HTMLElement | null>; model: TimelineModel }) {
  // Keep the last tip while closed so the tooltip fades out where it was.
  const [{ tip, open }, setState] = useState<{ tip: Tip | null; open: boolean }>({ tip: null, open: false })
  const show = (t: Tip) => setState({ tip: t, open: true })
  const hideTip = () => setState((s) => (s.open ? { ...s, open: false } : s))
  const modelRef = useRef(model)
  useEffect(() => {
    modelRef.current = model
  })

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const move = (e: PointerEvent) => {
      if (e.buttons !== 0) return hideTip()
      const target = (e.target as Element | null)?.closest?.('[data-tip]')
      const lines = target ? describeTip(modelRef.current, target) : null
      if (!lines) return hideTip()
      const box = el.getBoundingClientRect()
      show({ x: e.clientX - box.left, y: e.clientY - box.top, lines })
    }
    const hide = () => hideTip()
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerleave', hide)
    el.addEventListener('pointerdown', hide)
    el.addEventListener('wheel', hide, { passive: true })
    return () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerleave', hide)
      el.removeEventListener('pointerdown', hide)
      el.removeEventListener('wheel', hide)
    }
  }, [containerRef])

  return (
    <Tooltip open={open}>
      <TooltipTrigger asChild>
        <span aria-hidden className="pointer-events-none absolute size-px" style={{ left: tip?.x ?? 0, top: tip?.y ?? 0 }} />
      </TooltipTrigger>
      <TooltipContent side="top" sideOffset={10} className="flex-col items-start gap-0.5" data-testid="plot-tooltip">
        {tip?.lines.map((l, i) => (
          <span key={i} className={i === 0 ? 'font-medium' : 'opacity-80'}>
            {l}
          </span>
        ))}
      </TooltipContent>
    </Tooltip>
  )
}
