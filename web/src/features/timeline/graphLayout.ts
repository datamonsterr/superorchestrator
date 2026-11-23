import type { RunState } from '@/api/generated'
import type { RunViewPublic } from '@/api/types'
import { formatDuration } from '@/lib/format'
import { laneCurrentState, laneTitle } from './layout'
export { ORCHESTRATOR_LANE } from './model'
import { isTerminalState, ORCHESTRATOR_LANE, type RunLane, type TimelineModel } from './model'

export type GraphGroupingMode = 'hierarchy' | 'status' | 'harness'

export interface GraphNode {
  id: string
  lane: RunLane
  run?: RunViewPublic
  title: string
  state: RunState
  isOrphan: boolean
  missingParentId?: string
  parentId: string | null
  childrenIds: string[]
  depth: number
  // Visual coordinates
  x: number
  y: number
  width: number
  height: number
  colIndex: number
  rowIndex: number
  // Computed metadata
  durationMs: number
  formattedDuration: string
  toolCount: number
  messageCount: number
  account?: string
  harness?: string
  model?: string
}

export interface GraphEdge {
  id: string
  source: string // parent id
  target: string // child id
  isOrphan: boolean
  isCycle?: boolean
  kind: 'parent-child' | 'message'
  path: string
  sourceX: number
  sourceY: number
  targetX: number
  targetY: number
}

export interface GraphColumn {
  id: string
  title: string
  colIndex: number
  x: number
  width: number
  count: number
}

export interface GraphLayoutResult {
  nodes: GraphNode[]
  nodeMap: Map<string, GraphNode>
  edges: GraphEdge[]
  columns: GraphColumn[]
  width: number
  height: number
}

export const NODE_WIDTH = 250
export const NODE_HEIGHT = 100
export const COL_GAP = 72
export const ROW_GAP = 28
export const PADDING = 32
export const HEADER_HEIGHT = 36

const STATE_GROUPS: { id: string; label: string; states: RunState[] }[] = [
  { id: 'active', label: 'Active', states: ['starting', 'running', 'waiting'] },
  { id: 'queued', label: 'Queued', states: ['queued'] },
  { id: 'attention', label: 'Needs Attention', states: ['stalled', 'escalated', 'pending_review'] },
  { id: 'succeeded', label: 'Succeeded', states: ['succeeded'] },
  { id: 'failed', label: 'Failed / Cancelled', states: ['failed', 'cancelled'] },
]

function computeDuration(lane: RunLane, state: RunState, now: number): number {
  let start = lane.run?.created_at ? Date.parse(lane.run.created_at) : NaN
  if (!Number.isFinite(start) && lane.segments.length > 0) {
    start = lane.segments[0].start
  }
  if (!Number.isFinite(start)) return 0

  if (!lane.terminal && !isTerminalState(state)) {
    return Math.max(0, now - start)
  }

  const lastSeg = lane.segments[lane.segments.length - 1]
  const snapshotEnd = Date.parse(lane.run?.finished_at ?? '')
  if (Number.isFinite(snapshotEnd) && snapshotEnd >= start) {
    const eventEnd = lastSeg && isTerminalState(lastSeg.state) ? lastSeg.end : start
    return Math.max(snapshotEnd, eventEnd) - start
  }
  return Math.max(0, (lastSeg?.end ?? start) - start)
}

/**
 * Creates curved cubic Bezier SVG path connecting source output anchor to target input anchor.
 */
export function buildBezierPath(x1: number, y1: number, x2: number, y2: number): string {
  if (x2 >= x1) {
    const dx = x2 - x1
    const cx1 = x1 + Math.max(28, dx * 0.45)
    const cx2 = x2 - Math.max(28, dx * 0.45)
    return `M ${x1} ${y1} C ${cx1} ${y1}, ${cx2} ${y2}, ${x2} ${y2}`
  }
  // Reverse direction: loop curve around
  const mx = Math.max(x1, x2) + 40
  return `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`
}

/**
 * Computes deterministic graph nodes, edges, and coordinates from TimelineModel.
 * Safely handles missing/orphan parents, parent cycles, and live updates.
 */
export function computeGraphLayout(
  model: TimelineModel,
  grouping: GraphGroupingMode = 'hierarchy',
  now: number = model.now,
): GraphLayoutResult {
  const lanes = model.lanes
  const laneMap = new Map(lanes.map((l) => [l.runId, l]))

  // 1. Build initial node descriptors
  const rawNodes = new Map<string, {
    lane: RunLane
    title: string
    state: RunState
    parentId: string | null
    isOrphan: boolean
    missingParentId?: string
    depth: number
    durationMs: number
    toolCount: number
    messageCount: number
    account?: string
    harness?: string
    model?: string
  }>()

  // Track message counts per lane from model.arrows
  const msgCounts = new Map<string, number>()
  for (const arrow of model.arrows) {
    msgCounts.set(arrow.from, (msgCounts.get(arrow.from) ?? 0) + 1)
    msgCounts.set(arrow.to, (msgCounts.get(arrow.to) ?? 0) + 1)
  }

  // Orchestrator lane is always the coordinator root
  const orchLane = laneMap.get(ORCHESTRATOR_LANE) ?? lanes[0]
  const orchActive = model.active
  const orchState: RunState = orchActive ? 'running' : 'succeeded'
  rawNodes.set(ORCHESTRATOR_LANE, {
    lane: orchLane,
    title: 'Orchestrator',
    state: orchState,
    parentId: null,
    isOrphan: false,
    depth: 0,
    durationMs: Math.max(0, model.tMax - model.tMin),
    toolCount: orchLane.ticks.length,
    messageCount: msgCounts.get(ORCHESTRATOR_LANE) ?? 0,
  })

  // Add agent run lanes
  for (const lane of lanes) {
    if (lane.runId === ORCHESTRATOR_LANE) continue
    const run = lane.run
    const state: RunState = laneCurrentState(lane) ?? run?.state ?? 'queued'
    const durationMs = computeDuration(lane, state, now)

    let parentId: string
    let isOrphan = false
    let missingParentId: string | undefined

    if (!run?.parent_run || run.parent_run === ORCHESTRATOR_LANE || run.parent_run === lane.runId) {
      // Direct child of orchestrator
      parentId = ORCHESTRATOR_LANE
    } else if (laneMap.has(run.parent_run)) {
      parentId = run.parent_run
    } else {
      // Orphan run! Missing parent run
      isOrphan = true
      missingParentId = run.parent_run
      parentId = ORCHESTRATOR_LANE
    }

    rawNodes.set(lane.runId, {
      lane,
      title: laneTitle(lane),
      state,
      parentId,
      isOrphan,
      missingParentId,
      depth: lane.depth,
      durationMs,
      toolCount: lane.ticks.length,
      messageCount: msgCounts.get(lane.runId) ?? 0,
      account: run?.account ?? undefined,
      harness: run?.harness ?? undefined,
      model: run?.model ?? undefined,
    })
  }

  // 2. Cycle detection and validation
  // Build parent -> children map while detecting any circular references.
  // Sort run IDs deterministically so that cycle breaking is independent of lane input order.
  const childrenMap = new Map<string, string[]>()
  for (const [id] of rawNodes) {
    childrenMap.set(id, [])
  }

  const sortedIds = [...rawNodes.keys()]
    .filter((id) => id !== ORCHESTRATOR_LANE)
    .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))

  for (const id of sortedIds) {
    const node = rawNodes.get(id)
    if (!node || !node.parentId) continue

    // Detect cycle by traversing up ancestry
    let curr: string | null = node.parentId
    let cycle = false
    const visited = new Set<string>([id])

    while (curr && curr !== ORCHESTRATOR_LANE) {
      if (visited.has(curr)) {
        cycle = true
        break
      }
      visited.add(curr)
      curr = rawNodes.get(curr)?.parentId ?? null
    }

    if (cycle) {
      // Break cycle deterministically: attach directly to Orchestrator
      node.parentId = ORCHESTRATOR_LANE
    }

    const siblings = childrenMap.get(node.parentId) ?? []
    siblings.push(id)
    childrenMap.set(node.parentId, siblings)
  }

  // Sort children deterministically by created_at then runId
  const getCreatedMs = (id: string) => {
    const r = rawNodes.get(id)?.lane.run
    return r?.created_at ? Date.parse(r.created_at) : Number.POSITIVE_INFINITY
  }
  for (const children of childrenMap.values()) {
    children.sort((a, b) => getCreatedMs(a) - getCreatedMs(b) || (a < b ? -1 : 1))
  }

  // 3. Assign columns and rows according to grouping mode
  interface NodePlacement {
    id: string
    colIndex: number
    rowIndex: number
  }
  const placements: NodePlacement[] = []
  const columns: GraphColumn[] = []

  if (grouping === 'hierarchy') {
    // Column 0: Orchestrator
    // Column 1: Root runs (depth 0)
    // Column 2+: Subagents (depth 1+)
    const depthMap = new Map<string, number>()
    depthMap.set(ORCHESTRATOR_LANE, 0)

    const assignDepth = (id: string, depth: number) => {
      depthMap.set(id, depth)
      for (const childId of childrenMap.get(id) ?? []) {
        assignDepth(childId, depth + 1)
      }
    }
    assignDepth(ORCHESTRATOR_LANE, 0)

    // Group nodes by computed level
    const levelBuckets = new Map<number, string[]>()
    for (const [id] of rawNodes) {
      const d = depthMap.get(id) ?? 1
      const bucket = levelBuckets.get(d) ?? []
      bucket.push(id)
      levelBuckets.set(d, bucket)
    }

    const maxLevel = Math.max(...levelBuckets.keys(), 0)
    for (let lvl = 0; lvl <= maxLevel; lvl++) {
      const ids = levelBuckets.get(lvl) ?? []
      // Sort nodes in bucket: preserve tree order of parent
      if (lvl > 1) {
        ids.sort((a, b) => {
          const parentA = rawNodes.get(a)?.parentId ?? ''
          const parentB = rawNodes.get(b)?.parentId ?? ''
          if (parentA !== parentB) {
            const bucketPrev = levelBuckets.get(lvl - 1) ?? []
            return bucketPrev.indexOf(parentA) - bucketPrev.indexOf(parentB)
          }
          return getCreatedMs(a) - getCreatedMs(b) || (a < b ? -1 : 1)
        })
      } else if (lvl === 1) {
        ids.sort((a, b) => getCreatedMs(a) - getCreatedMs(b) || (a < b ? -1 : 1))
      }

      const colTitle = lvl === 0 ? 'Coordinator' : lvl === 1 ? 'Top-level Runs' : `Subagents (Depth ${lvl - 1})`
      columns.push({
        id: `col-${lvl}`,
        title: colTitle,
        colIndex: lvl,
        x: PADDING + lvl * (NODE_WIDTH + COL_GAP),
        width: NODE_WIDTH,
        count: ids.length,
      })

      ids.forEach((id, rowIndex) => {
        placements.push({ id, colIndex: lvl, rowIndex })
      })
    }
  } else if (grouping === 'status') {
    // Group by status
    STATE_GROUPS.forEach((group, colIndex) => {
      const matchIds = Array.from(rawNodes.keys()).filter((id) => {
        if (id === ORCHESTRATOR_LANE) return colIndex === 0 // place orchestrator in Active column
        const st = rawNodes.get(id)!.state
        return group.states.includes(st)
      })
      matchIds.sort((a, b) => getCreatedMs(a) - getCreatedMs(b) || (a < b ? -1 : 1))

      columns.push({
        id: group.id,
        title: group.label,
        colIndex,
        x: PADDING + colIndex * (NODE_WIDTH + COL_GAP),
        width: NODE_WIDTH,
        count: matchIds.length,
      })

      matchIds.forEach((id, rowIndex) => {
        placements.push({ id, colIndex, rowIndex })
      })
    })
  } else {
    // Group by harness
    const harnessSet = new Set<string>()
    for (const [id, n] of rawNodes) {
      if (id !== ORCHESTRATOR_LANE) {
        harnessSet.add(n.harness ?? 'other')
      }
    }
    const harnesses = ['orchestrator', ...Array.from(harnessSet).sort()]
    harnesses.forEach((h, colIndex) => {
      const matchIds = Array.from(rawNodes.keys()).filter((id) => {
        if (h === 'orchestrator') return id === ORCHESTRATOR_LANE
        return id !== ORCHESTRATOR_LANE && (rawNodes.get(id)!.harness ?? 'other') === h
      })
      matchIds.sort((a, b) => getCreatedMs(a) - getCreatedMs(b) || (a < b ? -1 : 1))

      columns.push({
        id: h,
        title: h === 'orchestrator' ? 'Coordinator' : `Harness: ${h}`,
        colIndex,
        x: PADDING + colIndex * (NODE_WIDTH + COL_GAP),
        width: NODE_WIDTH,
        count: matchIds.length,
      })

      matchIds.forEach((id, rowIndex) => {
        placements.push({ id, colIndex, rowIndex })
      })
    })
  }

  // 4. Construct final GraphNode records with pixel coordinates
  const nodes: GraphNode[] = []
  const nodeMap = new Map<string, GraphNode>()

  for (const { id, colIndex, rowIndex } of placements) {
    const raw = rawNodes.get(id)!
    const col = columns[colIndex]
    const x = col.x
    const y = PADDING + HEADER_HEIGHT + rowIndex * (NODE_HEIGHT + ROW_GAP)

    const gn: GraphNode = {
      id,
      lane: raw.lane,
      run: raw.lane.run,
      title: raw.title,
      state: raw.state,
      isOrphan: raw.isOrphan,
      missingParentId: raw.missingParentId,
      parentId: raw.parentId,
      childrenIds: childrenMap.get(id) ?? [],
      depth: raw.depth,
      x,
      y,
      width: NODE_WIDTH,
      height: NODE_HEIGHT,
      colIndex,
      rowIndex,
      durationMs: raw.durationMs,
      formattedDuration: formatDuration(raw.durationMs),
      toolCount: raw.toolCount,
      messageCount: raw.messageCount,
      account: raw.account,
      harness: raw.harness,
      model: raw.model,
    }
    nodes.push(gn)
    nodeMap.set(id, gn)
  }

  // 5. Construct edges with anchor coordinates and paths
  const edges: GraphEdge[] = []
  for (const node of nodes) {
    if (!node.parentId) continue
    const parentNode = nodeMap.get(node.parentId)
    if (!parentNode) continue

    const sourceX = parentNode.x + parentNode.width
    const sourceY = parentNode.y + parentNode.height / 2
    const targetX = node.x
    const targetY = node.y + node.height / 2

    const path = buildBezierPath(sourceX, sourceY, targetX, targetY)
    edges.push({
      id: `edge-${parentNode.id}-${node.id}`,
      source: parentNode.id,
      target: node.id,
      isOrphan: node.isOrphan,
      kind: 'parent-child',
      path,
      sourceX,
      sourceY,
      targetX,
      targetY,
    })
  }

  // 6. Compute canvas total width and height
  const maxX = Math.max(...nodes.map((n) => n.x + n.width), columns[columns.length - 1]?.x + NODE_WIDTH || 800)
  const maxY = Math.max(...nodes.map((n) => n.y + n.height), 400)
  const width = maxX + PADDING
  const height = maxY + PADDING

  return { nodes, nodeMap, edges, columns, width, height }
}
