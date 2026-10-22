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
    // At most one update per animation frame, from the latest pointer position.
    let frame = 0
    let last: { target: Element | null; x: number; y: number; buttons: number } | null = null
    const run = () => {
      frame = 0
      if (!last) return
      const { target, x, y, buttons } = last
      if (buttons !== 0) return hideTip()
      const hit = target?.closest?.('[data-tip]')
      const lines = hit ? describeTip(modelRef.current, hit) : null
      if (!lines) return hideTip()
      const box = el.getBoundingClientRect()
      show({ x: x - box.left, y: y - box.top, lines })
    }
    const move = (e: PointerEvent) => {
      last = { target: e.target as Element | null, x: e.clientX, y: e.clientY, buttons: e.buttons }
      if (!frame) frame = requestAnimationFrame(run)
    }
    const hide = () => {
      last = null
      if (frame) cancelAnimationFrame(frame)
      frame = 0
      hideTip()
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerleave', hide)
    el.addEventListener('pointerdown', hide)
    el.addEventListener('wheel', hide, { passive: true })
    return () => {
      if (frame) cancelAnimationFrame(frame)
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
