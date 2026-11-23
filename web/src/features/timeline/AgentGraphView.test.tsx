import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { RunState } from '@/api/generated'
import type { Event, RunViewPublic } from '@/api/types'
import { applyEvent } from './applyEvent'
import { deriveTimeline } from './derive'
import { AgentGraphView } from './AgentGraphView'

function makeRun(id: string, name: string, parentRun: string | null = null, depth = 0, state: RunState = 'running'): RunViewPublic {
  return {
    run_id: id,
    session_id: 'sess-test',
    name,
    parent_run: parentRun,
    depth,
    state,
    harness: 'agy',
    account: 'acc-1',
    model: 'model-a',
    created_at: new Date(Date.UTC(2026, 0, 1, 12, 0)).toISOString(),
    finished_at: state === 'succeeded' ? new Date(Date.UTC(2026, 0, 1, 12, 5)).toISOString() : null,
    recent_events: [],
    usage: { input_tokens: 100, output_tokens: 50, cost: 0.01 },
  }
}

describe('AgentGraphView', () => {
  it('renders coordinator and agent nodes with state, title, and metadata', () => {
    const runs = [
      makeRun('run-planner', 'planner', null, 0, 'succeeded'),
      makeRun('run-coder', 'coder', 'run-planner', 1, 'running'),
    ]
    const events: Event[] = [
      {
        id: 'ev-tool-1',
        session_id: 'sess-test',
        run_id: 'run-coder',
        ts: new Date(Date.UTC(2026, 0, 1, 12, 2)).toISOString(),
        type: 'tool_call_started',
        data: { name: 'edit_file' },
      },
    ]
    const model = deriveTimeline(runs, events, Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    expect(screen.getByTestId('agent-graph')).toBeInTheDocument()
    expect(screen.getByTestId('graph-node-orchestrator')).toBeInTheDocument()
    expect(screen.getByTestId('graph-node-run-planner')).toBeInTheDocument()
    expect(screen.getByTestId('graph-node-run-coder')).toBeInTheDocument()

    // Title and duration
    expect(screen.getByText('planner')).toBeInTheDocument()
    expect(screen.getByText('coder')).toBeInTheDocument()
    expect(screen.getByText('5m 0s')).toBeInTheDocument() // planner finished in 5m
    expect(screen.getByText('run-coder')).toBeInTheDocument()
    // Tool calls count
    expect(screen.getByTitle('1 tool calls')).toBeInTheDocument()
  })

  it('calls onSelect when a node is clicked', async () => {
    const user = userEvent.setup()
    const runs = [makeRun('run-planner', 'planner', null, 0, 'succeeded')]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    await user.click(screen.getByTestId('graph-node-run-planner'))
    expect(onSelect).toHaveBeenCalledWith('run-planner')

    // Clicking orchestrator should not trigger run selection
    onSelect.mockClear()
    await user.click(screen.getByTestId('graph-node-orchestrator'))
    expect(onSelect).not.toHaveBeenCalled()
  })

  it('supports keyboard navigation and selection with Enter', async () => {
    const user = userEvent.setup()
    const runs = [
      makeRun('run-1', 'agent-1', null, 0, 'succeeded'),
      makeRun('run-2', 'agent-2', 'run-1', 1, 'running'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    const orchNode = screen.getByTestId('graph-node-orchestrator')
    orchNode.focus()
    expect(orchNode).toHaveFocus()

    // Press ArrowRight to navigate to child run-1
    await user.keyboard('{ArrowRight}')
    const node1 = screen.getByTestId('graph-node-run-1')
    expect(node1).toHaveFocus()

    // Press Enter to select run-1
    await user.keyboard('{Enter}')
    expect(onSelect).toHaveBeenCalledWith('run-1')

    // Press ArrowRight to navigate to subagent run-2
    await user.keyboard('{ArrowRight}')
    const node2 = screen.getByTestId('graph-node-run-2')
    expect(node2).toHaveFocus()

    // Press ArrowLeft to return to run-1
    await user.keyboard('{ArrowLeft}')
    expect(node1).toHaveFocus()
  })

  it('handles orphan run with missing parent and displays orphan badge', () => {
    const runs = [
      makeRun('run-orphan', 'orphan-agent', 'missing-parent-id', 1, 'stalled'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    const orphanNode = screen.getByTestId('graph-node-run-orphan')
    expect(orphanNode).toBeInTheDocument()
    expect(screen.getByText('Orphan')).toBeInTheDocument()
    expect(orphanNode).toHaveAttribute('aria-label', expect.stringContaining('orphan'))

    // Edge is marked orphan
    const edge = screen.getByTestId('edge-edge-orchestrator-run-orphan')
    expect(edge).toHaveAttribute('stroke-dasharray', '5 4')
  })

  it('dims non-matching nodes when search or state filter is applied', () => {
    const runs = [
      makeRun('run-1', 'planner', null, 0, 'succeeded'),
      makeRun('run-2', 'coder', 'run-1', 1, 'running'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    const { rerender } = render(
      <AgentGraphView model={model} onSelect={onSelect} searchQuery="coder" />
    )

    const node1 = screen.getByTestId('graph-node-run-1')
    const node2 = screen.getByTestId('graph-node-run-2')

    // run-1 should be dimmed, run-2 should match
    expect(node1.className).toContain('opacity-25')
    expect(node2.className).not.toContain('opacity-25')

    // State filter: only succeeded
    rerender(
      <AgentGraphView model={model} onSelect={onSelect} stateFilter="succeeded" />
    )
    expect(node1.className).not.toContain('opacity-25')
    expect(node2.className).toContain('opacity-25')
  })

  it('allows switching grouping mode', async () => {
    const user = userEvent.setup()
    const runs = [
      makeRun('run-1', 'task-1', null, 0, 'running'),
      makeRun('run-2', 'task-2', null, 0, 'succeeded'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    const groupBtn = screen.getByRole('button', { name: 'Group graph nodes' })
    expect(groupBtn).toHaveTextContent('Hierarchy')

    await user.click(groupBtn)
    await user.click(screen.getByText('By Status'))

    expect(screen.getByRole('button', { name: 'Group graph nodes' })).toHaveTextContent('By Status')
    expect(screen.getAllByText('Succeeded').length).toBeGreaterThanOrEqual(2)
  })

  it('zooms in, out, fits, and resets zoom', async () => {
    const user = userEvent.setup()
    const runs = [makeRun('run-1', 'single')]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    expect(screen.getByText('100%')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Zoom in' }))
    expect(screen.getByText('120%')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Zoom out' }))
    expect(screen.getByText('100%')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Reset zoom to 100%' }))
    expect(screen.getByText('100%')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Fit graph to window' }))
    expect(screen.getByText(/\d+%/)).toBeInTheDocument()
  })

  it('toggles message arrows display', async () => {
    const user = userEvent.setup()
    const runs = [
      makeRun('run-1', 'sender'),
      makeRun('run-2', 'receiver'),
    ]
    const events: Event[] = [
      {
        id: 'ev-msg-1',
        session_id: 'sess-test',
        run_id: 'run-1',
        ts: new Date(Date.UTC(2026, 0, 1, 12, 1)).toISOString(),
        type: 'message_sent',
        data: { to_run: 'run-2', kind: 'progress' },
      },
    ]
    const model = deriveTimeline(runs, events, Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    const msgBtn = screen.getByRole('button', { name: /Messages/ })
    expect(msgBtn).toHaveAttribute('aria-pressed', 'false')

    await user.click(msgBtn)
    expect(msgBtn).toHaveAttribute('aria-pressed', 'true')
  })

  it('renders run cards as native buttons in a valid list structure with roving keyboard focus', async () => {
    const user = userEvent.setup()
    const runs = [
      makeRun('run-1', 'agent-one', null, 0, 'running'),
      makeRun('run-2', 'agent-two', 'run-1', 1, 'succeeded'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    render(<AgentGraphView model={model} onSelect={onSelect} />)

    // Valid list semantics: ul > li > button
    const list = screen.getByRole('list', { name: 'Agents list' })
    expect(list.tagName).toBe('UL')

    const listItems = within(list).getAllByRole('listitem')
    expect(listItems.length).toBe(3) // Orchestrator + run-1 + run-2
    for (const item of listItems) {
      expect(item.tagName).toBe('LI')
    }

    // Run nodes are native button elements with accessible names
    const btn1 = screen.getByRole('button', { name: /agent-one/i })
    expect(btn1.tagName).toBe('BUTTON')
    expect(btn1).toHaveAttribute('type', 'button')

    const btn2 = screen.getByRole('button', { name: /agent-two/i })
    expect(btn2.tagName).toBe('BUTTON')

    // Click triggers selection
    await user.click(btn1)
    expect(onSelect).toHaveBeenCalledWith('run-1')

    // Keyboard roving focus
    btn1.focus()
    expect(btn1).toHaveFocus()
    await user.keyboard('{ArrowRight}')
    expect(btn2).toHaveFocus()
    await user.keyboard('{Enter}')
    expect(onSelect).toHaveBeenCalledWith('run-2')
  })

  it('displays authoritative current snapshot state when history is incomplete and ensures newer live transitions win', async () => {
    // API snapshot has state: pending_review (like write-tests in fixtures),
    // but event history only has run_queued and escalated marker (omits state_changed to pending_review)
    const runPending = makeRun('run-write-tests', 'write-tests', null, 0, 'pending_review')
    const initialEvents: Event[] = [
      {
        id: 'ev-1',
        session_id: 'sess-test',
        run_id: 'run-write-tests',
        ts: new Date(Date.UTC(2026, 0, 1, 12, 0)).toISOString(),
        type: 'run_queued',
        data: {},
      },
      {
        id: 'ev-2',
        session_id: 'sess-test',
        run_id: 'run-write-tests',
        ts: new Date(Date.UTC(2026, 0, 1, 12, 2)).toISOString(),
        type: 'escalated',
        data: { reason: 'stalled' },
      },
    ]

    const now = Date.UTC(2026, 0, 1, 12, 10)
    const initialModel = deriveTimeline([runPending], initialEvents, now)
    const onSelect = vi.fn()

    const { rerender } = render(<AgentGraphView model={initialModel} onSelect={onSelect} />)

    // Displays authoritative snapshot state 'Pending review' on the card, not 'Queued'
    const card = screen.getByTestId('graph-node-run-write-tests')
    expect(card).toHaveAttribute('aria-label', expect.stringContaining('state pending_review'))
    expect(within(card).getByText('Pending review')).toBeInTheDocument()
    expect(within(card).queryByText('Queued')).toBeNull()

    // Now a newer live transition arrives (e.g. review rejected, retrying -> running)
    const liveTransitionEvent: Event = {
      id: 'ev-3',
      session_id: 'sess-test',
      run_id: 'run-write-tests',
      ts: new Date(Date.UTC(2026, 0, 1, 12, 5)).toISOString(),
      type: 'state_changed',
      data: { from: 'pending_review', to: 'running' },
    }
    const updatedModel = applyEvent(initialModel, liveTransitionEvent, now + 1000)

    rerender(<AgentGraphView model={updatedModel} onSelect={onSelect} />)

    // The newer live transition MUST win
    const updatedCard = screen.getByTestId('graph-node-run-write-tests')
    expect(updatedCard).toHaveAttribute('aria-label', expect.stringContaining('state running'))
    expect(within(updatedCard).getByText('Running')).toBeInTheDocument()
    expect(within(updatedCard).queryByText('Pending review')).toBeNull()
  })

  it('follows focusedNodeId ahead of selectedRunId to preserve roving focus after navigating away', async () => {
    const user = userEvent.setup()
    const runs = [
      makeRun('run-1', 'agent-one', null, 0, 'running'),
      makeRun('run-2', 'agent-two', 'run-1', 1, 'running'),
    ]
    const model = deriveTimeline(runs, [], Date.UTC(2026, 0, 1, 12, 10))
    const onSelect = vi.fn()

    // Initially run-1 is selected via prop
    render(<AgentGraphView model={model} selectedRunId="run-1" onSelect={onSelect} />)

    const btn1 = screen.getByTestId('graph-node-run-1')
    const btn2 = screen.getByTestId('graph-node-run-2')

    // Initial state: run-1 is selected and has roving tab stop
    expect(btn1).toHaveAttribute('tabIndex', '0')
    expect(btn2).toHaveAttribute('tabIndex', '-1')

    // User navigates away to run-2 using keyboard
    btn1.focus()
    await user.keyboard('{ArrowRight}')

    // Focus moved to run-2
    expect(btn2).toHaveFocus()

    // effectiveFocusId must follow focusedNodeId (run-2), NOT stay on selectedRunId (run-1)
    expect(btn2).toHaveAttribute('tabIndex', '0')
    expect(btn1).toHaveAttribute('tabIndex', '-1')
    expect(btn2.className).toContain('ring-ring')
  })
})
