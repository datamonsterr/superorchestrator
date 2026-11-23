import { describe, expect, it } from 'vitest'
import type { RunState } from '@/api/generated'
import type { Event, RunViewPublic } from '@/api/types'
import { applyEvent } from './applyEvent'
import { deriveTimeline } from './derive'
import { computeGraphLayout, NODE_HEIGHT, NODE_WIDTH, ORCHESTRATOR_LANE } from './graphLayout'

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

describe('graphLayout', () => {
  it('creates coordinator root and top-level runs in hierarchy layout', () => {
    const runs = [
      makeRun('run-1', 'planner', null, 0, 'succeeded'),
      makeRun('run-2', 'coder', 'run-1', 1, 'running'),
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'hierarchy', now)

    expect(layout.nodes).toHaveLength(3) // orchestrator, run-1, run-2
    expect(layout.nodeMap.has(ORCHESTRATOR_LANE)).toBe(true)
    expect(layout.nodeMap.has('run-1')).toBe(true)
    expect(layout.nodeMap.has('run-2')).toBe(true)

    const orch = layout.nodeMap.get(ORCHESTRATOR_LANE)!
    const r1 = layout.nodeMap.get('run-1')!
    const r2 = layout.nodeMap.get('run-2')!

    // Check levels
    expect(orch.colIndex).toBe(0)
    expect(r1.colIndex).toBe(1)
    expect(r2.colIndex).toBe(2)

    // Check parent links
    expect(r1.parentId).toBe(ORCHESTRATOR_LANE)
    expect(r2.parentId).toBe('run-1')
    expect(orch.childrenIds).toContain('run-1')
    expect(r1.childrenIds).toContain('run-2')

    // Check edges
    expect(layout.edges).toHaveLength(2)
    const edgeOrchR1 = layout.edges.find((e) => e.source === ORCHESTRATOR_LANE && e.target === 'run-1')
    const edgeR1R2 = layout.edges.find((e) => e.source === 'run-1' && e.target === 'run-2')

    expect(edgeOrchR1).toBeDefined()
    expect(edgeR1R2).toBeDefined()
    expect(edgeOrchR1?.isOrphan).toBe(false)
    expect(edgeR1R2?.path).toContain('M')
  })

  it('handles orphan runs with missing parent gracefully', () => {
    const runs = [
      makeRun('run-child', 'subagent', 'run-non-existent-parent', 1, 'running'),
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'hierarchy', now)

    const child = layout.nodeMap.get('run-child')!
    expect(child).toBeDefined()
    expect(child.isOrphan).toBe(true)
    expect(child.missingParentId).toBe('run-non-existent-parent')
    expect(child.parentId).toBe(ORCHESTRATOR_LANE)

    const edge = layout.edges.find((e) => e.target === 'run-child')!
    expect(edge).toBeDefined()
    expect(edge.isOrphan).toBe(true)
    expect(edge.source).toBe(ORCHESTRATOR_LANE)
  })

  it('handles cyclical parent links deterministically without infinite loop', () => {
    const runs = [
      makeRun('run-a', 'agent-a', 'run-b', 1, 'running'),
      makeRun('run-b', 'agent-b', 'run-a', 1, 'running'),
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'hierarchy', now)

    expect(layout.nodes).toHaveLength(3) // orchestrator, run-a, run-b
    expect(layout.nodeMap.has('run-a')).toBe(true)
    expect(layout.nodeMap.has('run-b')).toBe(true)
    // The cycle must be broken deterministically
    expect(layout.edges.length).toBeGreaterThanOrEqual(1)
  })

  it('computes status grouping mode correctly', () => {
    const runs = [
      makeRun('run-1', 'task-1', null, 0, 'running'),
      makeRun('run-2', 'task-2', null, 0, 'succeeded'),
      makeRun('run-3', 'task-3', null, 0, 'failed'),
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'status', now)

    expect(layout.columns.length).toBe(5) // active, queued, attention, succeeded, failed
    const r1 = layout.nodeMap.get('run-1')!
    const r2 = layout.nodeMap.get('run-2')!
    const r3 = layout.nodeMap.get('run-3')!

    expect(r1.colIndex).toBe(0) // active
    expect(r2.colIndex).toBe(3) // succeeded
    expect(r3.colIndex).toBe(4) // failed
  })

  it('computes harness grouping mode correctly', () => {
    const runs: RunViewPublic[] = [
      { ...makeRun('run-1', 'task-1'), harness: 'agy' },
      { ...makeRun('run-2', 'task-2'), harness: 'claude' },
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'harness', now)

    expect(layout.columns.length).toBeGreaterThanOrEqual(2)
    const r1 = layout.nodeMap.get('run-1')!
    const r2 = layout.nodeMap.get('run-2')!

    expect(r1.harness).toBe('agy')
    expect(r2.harness).toBe('claude')
    expect(r1.colIndex).not.toBe(r2.colIndex)
  })

  it('computes positive layout dimensions and edge coordinates', () => {
    const runs = [makeRun('run-1', 'single-run')]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, [], now)

    const layout = computeGraphLayout(model, 'hierarchy', now)

    expect(layout.width).toBeGreaterThan(NODE_WIDTH)
    expect(layout.height).toBeGreaterThan(NODE_HEIGHT)

    for (const edge of layout.edges) {
      expect(edge.sourceX).toBeGreaterThanOrEqual(0)
      expect(edge.sourceY).toBeGreaterThanOrEqual(0)
      expect(edge.targetX).toBeGreaterThanOrEqual(0)
      expect(edge.targetY).toBeGreaterThanOrEqual(0)
      expect(edge.path).toMatch(/^M \d+(\.\d+)? \d+(\.\d+)? C/)
    }
  })

  it('displays authoritative current snapshot state when history is incomplete and newer live transitions win', () => {
    // Snapshot state is pending_review, but event history only has run_queued
    const runs = [makeRun('run-review', 'agent-review', null, 0, 'pending_review')]
    const initialEvents: Event[] = [
      {
        id: 'ev-1',
        session_id: 'sess-test',
        run_id: 'run-review',
        ts: new Date(Date.UTC(2026, 0, 1, 12, 0)).toISOString(),
        type: 'run_queued',
        data: {},
      },
    ]
    const now = Date.UTC(2026, 0, 1, 12, 10)
    const model = deriveTimeline(runs, initialEvents, now)

    const layout = computeGraphLayout(model, 'hierarchy', now)
    const node = layout.nodeMap.get('run-review')!
    expect(node.state).toBe('pending_review')

    // In status grouping, pending_review falls into attention column (colIndex: 2)
    const statusLayout = computeGraphLayout(model, 'status', now)
    const statusNode = statusLayout.nodeMap.get('run-review')!
    expect(statusNode.colIndex).toBe(2) // attention column

    // Now a newer live transition arrives (succeeded)
    const liveEvent: Event = {
      id: 'ev-2',
      session_id: 'sess-test',
      run_id: 'run-review',
      ts: new Date(Date.UTC(2026, 0, 1, 12, 5)).toISOString(),
      type: 'run_finished',
      data: { status: 'succeeded' },
    }
    const updatedModel = applyEvent(model, liveEvent, now + 1000)
    const updatedLayout = computeGraphLayout(updatedModel, 'hierarchy', now + 1000)
    const updatedNode = updatedLayout.nodeMap.get('run-review')!
    expect(updatedNode.state).toBe('succeeded')
  })
})
