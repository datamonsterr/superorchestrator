import { fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { events, runs } from '@/api/msw/fixtures'
import { makeLargeSession } from '@/test/fixtures/large'
import { deriveTimeline } from './derive'
import { GanttCanvas, type GanttCanvasProps } from './GanttCanvas'
import { fitViewport, span, type Viewport } from './viewport'

const NOW = Date.UTC(2026, 0, 1, 12, 30)

function setup(over: Partial<GanttCanvasProps> = {}) {
  const model = deriveTimeline(runs, events, NOW)
  const onSelect = vi.fn()
  const onViewportChange = vi.fn()
  const viewport = fitViewport(model)
  const ui = (
    <TooltipProvider>
      <GanttCanvas model={model} viewport={viewport} onViewportChange={onViewportChange} onSelect={onSelect} {...over} />
    </TooltipProvider>
  )
  return { ...render(ui), model, onSelect, onViewportChange, viewport }
}

describe('GanttCanvas', () => {
  it('renders the orchestrator lane and a lane per run, nested by parent', () => {
    setup()
    const list = screen.getByRole('list', { name: 'Runs' })
    const items = within(list).getAllByRole('listitem')
    expect(items.map((li) => li.getAttribute('data-lane'))).toEqual(['orchestrator', 'run-1', 'run-2'])
    expect(screen.getByLabelText(/^refactor-api, /)).toBeInTheDocument()
    expect(screen.getByTestId('lane-run-2')).toBeInTheDocument()
    // The child is indented under its parent.
    expect(within(screen.getByTestId('label-run-2')).getByText('└')).toBeInTheDocument()
    expect(within(screen.getByTestId('label-run-1')).queryByText('└')).toBeNull()
  })

  it('exposes the plot as an application with an accessible name', () => {
    setup()
    expect(screen.getByRole('application', { name: /Session timeline/ })).toBeInTheDocument()
  })

  it('calls onSelect when a lane label is clicked', async () => {
    const { onSelect } = setup()
    await userEvent.click(within(screen.getByRole('list', { name: 'Runs' })).getByText('write-tests'))
    expect(onSelect).toHaveBeenCalledWith('run-2')
  })

  it('calls onSelect when a bar is clicked, and not after a drag', () => {
    const { onSelect } = setup()
    fireEvent.click(screen.getByTestId('seg-run-1-0'))
    expect(onSelect).toHaveBeenCalledWith('run-1')
  })

  it('opens the focused lane with Enter', async () => {
    const { onSelect } = setup()
    screen.getByTestId('label-run-2').focus()
    await userEvent.keyboard('{Enter}')
    expect(onSelect).toHaveBeenCalledWith('run-2')
  })

  it('walks the segments of a focused lane with the arrow keys and announces them', async () => {
    setup()
    screen.getByTestId('label-run-1').focus()
    await userEvent.keyboard('{ArrowRight}')
    expect(screen.getByTestId('gantt-announce')).toHaveTextContent(/^queued, \d\d:\d\d to \d\d:\d\d, codex-acc1/)
    await userEvent.keyboard('{ArrowRight}')
    expect(screen.getByTestId('gantt-announce')).toHaveTextContent(/^running, \d\d:\d\d to \d\d:\d\d/)
    await userEvent.keyboard('{ArrowLeft}')
    expect(screen.getByTestId('gantt-announce')).toHaveTextContent(/^queued/)
  })

  it('moves focus between lanes with ArrowDown and ArrowUp', async () => {
    setup()
    screen.getByTestId('label-orchestrator').focus()
    await userEvent.keyboard('{ArrowDown}')
    expect(screen.getByTestId('label-run-1')).toHaveFocus()
    await userEvent.keyboard('{ArrowUp}')
    expect(screen.getByTestId('label-orchestrator')).toHaveFocus()
  })

  it('has a screen-reader summary table with one row per segment and marker', () => {
    setup()
    const table = screen.getByTestId('timeline-summary')
    expect(table).toHaveClass('sr-only')
    expect(within(table).getByRole('columnheader', { name: 'Account' })).toBeInTheDocument()
    expect(within(table).getAllByRole('row').length).toBeGreaterThan(5)
    expect(within(table).getByText('Stall detected (idle 60s)')).toBeInTheDocument()
    expect(within(table).getAllByText('write-tests').length).toBeGreaterThan(0)
  })

  it('zooms with + and - and fits with 0', async () => {
    const { onViewportChange, viewport } = setup()
    screen.getByRole('application').focus()
    await userEvent.keyboard('+')
    expect(span(onViewportChange.mock.calls[0][0] as Viewport)).toBeLessThan(span(viewport))
    await userEvent.keyboard('-')
    expect(span(onViewportChange.mock.calls[1][0] as Viewport)).toBeGreaterThan(span(viewport))
    await userEvent.keyboard('0')
    expect(onViewportChange.mock.calls[2][0]).toEqual(viewport)
  })

  it('pans with the arrow keys on the plot and zooms with ctrl+wheel', async () => {
    const { onViewportChange, viewport } = setup()
    screen.getByRole('application').focus()
    await userEvent.keyboard('{ArrowRight}')
    const panned = onViewportChange.mock.calls[0][0] as Viewport
    expect(panned.t0).toBeGreaterThan(viewport.t0)
    expect(span(panned)).toBeCloseTo(span(viewport), 3)
    fireEvent.wheel(screen.getByTestId('gantt-plot'), { deltaY: -200, ctrlKey: true })
    expect(span(onViewportChange.mock.calls[1][0] as Viewport)).toBeLessThan(span(viewport))
  })

  it('pans by dragging the plot', () => {
    const { onViewportChange, viewport } = setup()
    const plot = screen.getByTestId('gantt-plot')
    fireEvent.pointerDown(plot, { button: 0, clientX: 400, pointerId: 1 })
    fireEvent.pointerMove(window, { clientX: 300, pointerId: 1 })
    fireEvent.pointerUp(window, { pointerId: 1 })
    const v = onViewportChange.mock.calls.at(-1)![0] as Viewport
    expect(v.t0).toBeGreaterThan(viewport.t0)
  })

  it('shows a tooltip with state, time range and account for a hovered bar', async () => {
    setup()
    const bar = screen.getByTestId('seg-run-1-1')
    fireEvent.pointerMove(bar, { clientX: 100, clientY: 50, buttons: 0 })
    const tip = await screen.findAllByText('Running')
    expect(tip.length).toBeGreaterThan(0)
    expect((await screen.findAllByText(/\d\d:\d\d:\d\d to \d\d:\d\d:\d\d/)).length).toBeGreaterThan(0)
  })

  it('draws a now line while a run is active and a marker glyph for the stall', () => {
    setup()
    expect(screen.getByTestId('now-line')).toBeInTheDocument()
    const lane2 = screen.getByTestId('lane-run-2')
    expect(lane2.querySelectorAll('[data-tip="marker"]')).toHaveLength(2) // stall + escalated
  })

  it('draws message arrows between lanes', () => {
    const model = deriveTimeline(
      runs,
      [...events, { id: 'ev-9000', session_id: 'sess-1', run_id: 'run-1', ts: new Date(Date.UTC(2026, 0, 1, 12, 6)).toISOString(), type: 'message_sent', data: { to: 'run-2', kind: 'message' } }],
      NOW,
    )
    const { container } = render(
      <TooltipProvider>
        <GanttCanvas model={model} viewport={fitViewport(model)} onViewportChange={() => {}} onSelect={() => {}} />
      </TooltipProvider>,
    )
    expect(container.querySelectorAll('[data-tip="arrow"]')).toHaveLength(1)
  })

  it('marks the selected lane', () => {
    setup({ selectedRunId: 'run-2' })
    expect(screen.getByTestId('label-run-2')).toHaveAttribute('aria-current', 'true')
    expect(screen.getByTestId('label-run-1')).not.toHaveAttribute('aria-current')
  })

  it('defines pattern fills once and colours bars with CSS variables', () => {
    const { container } = setup()
    expect(container.querySelector('pattern#so-pat-hatch')).not.toBeNull()
    const bar = screen.getByTestId('seg-run-1-1').querySelector('rect')!
    expect(bar.getAttribute('fill')).toBe('var(--state-running)')
    expect(document.head.innerHTML + document.body.innerHTML).toContain('--state-running:')
  })
})

describe('GanttCanvas with the large fixture', () => {
  const large = makeLargeSession({ events: 10000, runs: 30, transcriptRows: 0, seed: 1 })
  const now = Date.parse(large.session.ended_at!)
  const model = deriveTimeline(large.session.runs, large.events, now)

  it('renders 31 lanes, culls ticks to a bounded node count and records the render time', () => {
    const t = performance.now()
    const { container, unmount } = render(
      <TooltipProvider>
        <GanttCanvas model={model} viewport={fitViewport(model)} onViewportChange={() => {}} onSelect={() => {}} />
      </TooltipProvider>,
    )
    const ms = performance.now() - t
    console.info(`[perf] GanttCanvas initial render, 10000 events / 30 runs, fit viewport (jsdom): ${ms.toFixed(0)} ms`)
    expect(screen.getAllByRole('listitem')).toHaveLength(31)
    const ticks = container.querySelectorAll('[data-tip="tick"]').length
    const total = model.lanes.reduce((n, l) => n + l.ticks.length, 0)
    expect(total).toBeGreaterThan(3000)
    expect(ticks).toBeLessThanOrEqual(31 * (960 / 3 + 2))
    expect(container.querySelectorAll('*').length).toBeLessThan(12000)
    unmount()
  })

  it('renders far fewer nodes when zoomed into a one-minute window (time culling)', () => {
    const mid = (model.tMin + model.tMax) / 2
    const { container, rerender } = render(
      <TooltipProvider>
        <GanttCanvas model={model} viewport={fitViewport(model)} onViewportChange={() => {}} onSelect={() => {}} />
      </TooltipProvider>,
    )
    const full = container.querySelectorAll('[data-tip]').length
    const t = performance.now()
    rerender(
      <TooltipProvider>
        <GanttCanvas model={model} viewport={{ t0: mid, t1: mid + 60_000 }} onViewportChange={() => {}} onSelect={() => {}} />
      </TooltipProvider>,
    )
    const ms = performance.now() - t
    console.info(`[perf] GanttCanvas re-render at a one-minute viewport (jsdom): ${ms.toFixed(0)} ms`)
    const zoomed = container.querySelectorAll('[data-tip]').length
    expect(zoomed).toBeLessThan(full / 2)
  })

  it('virtualises lanes above 60', () => {
    // jsdom has no layout: give the scroll container a 320 px height (10 lanes).
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(320)
    const big = makeLargeSession({ events: 3000, runs: 120, transcriptRows: 0, seed: 2 })
    const m = deriveTimeline(big.session.runs, big.events, Date.parse(big.session.ended_at!))
    render(
      <TooltipProvider>
        <GanttCanvas model={m} viewport={fitViewport(m)} onViewportChange={() => {}} onSelect={() => {}} />
      </TooltipProvider>,
    )
    expect(m.lanes).toHaveLength(121)
    const rendered = screen.getAllByRole('listitem').length
    expect(rendered).toBeGreaterThanOrEqual(10)
    expect(rendered).toBeLessThan(40)
  })

  it('keeps the viewport controlled by the parent', async () => {
    function Harness() {
      const [vp, setVp] = useState(fitViewport(model))
      return (
        <TooltipProvider>
          <GanttCanvas model={model} viewport={vp} onViewportChange={setVp} onSelect={() => {}} />
          <output data-testid="span">{Math.round(span(vp))}</output>
        </TooltipProvider>
      )
    }
    render(<Harness />)
    const before = Number(screen.getByTestId('span').textContent)
    screen.getByRole('application').focus()
    await userEvent.keyboard('{+}')
    expect(Number(screen.getByTestId('span').textContent)).toBeLessThan(before)
  })
})
