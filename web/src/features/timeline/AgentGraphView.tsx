import { useCallback, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Clock,
  Cpu,
  GitFork,
  Layers,
  Maximize,
  MessageSquare,
  Minus,
  Plus,
  RotateCcw,
  Wrench,
  X,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { STATE_STYLES, stateFill } from './colors'
import {
  computeGraphLayout,
  ORCHESTRATOR_LANE,
  type GraphGroupingMode,
  type GraphNode,
} from './graphLayout'
import { type TimelineModel } from './model'
import { cn } from '@/lib/utils'

export interface AgentGraphViewProps {
  model: TimelineModel
  selectedRunId?: string | null
  onSelect: (runId: string) => void
  searchQuery?: string
  stateFilter?: string
  className?: string
}

export function AgentGraphView({
  model,
  selectedRunId,
  onSelect,
  searchQuery = '',
  stateFilter = 'all',
  className,
}: AgentGraphViewProps) {
  const [grouping, setGrouping] = useState<GraphGroupingMode>('hierarchy')
  const [zoom, setZoom] = useState<number>(1)
  const [hoveredNodeId, setHoveredNodeId] = useState<string | null>(null)
  const [focusedNodeId, setFocusedNodeId] = useState<string | null>(null)
  const [showMessages, setShowMessages] = useState<boolean>(false)
  const [liveAnnouncement, setLiveAnnouncement] = useState<string>('')
  const containerRef = useRef<HTMLDivElement>(null)
  const graphHelpId = useId()

  // Compute graph layout deterministically
  const layout = useMemo(
    () => computeGraphLayout(model, grouping, model.now),
    [model, grouping],
  )

  // Filter matching evaluation
  const queryLower = searchQuery.trim().toLowerCase()
  const isNodeMatch = useCallback(
    (node: GraphNode): boolean => {
      // Orchestrator node matches if no strict query, or if query is 'orchestrator'
      if (node.id === ORCHESTRATOR_LANE) {
        if (!queryLower && stateFilter === 'all') return true
        if (queryLower && 'orchestrator coordinator'.includes(queryLower)) return true
        return !queryLower
      }

      // Check state filter
      if (stateFilter !== 'all') {
        if (stateFilter === 'active' && !['starting', 'running', 'waiting'].includes(node.state)) return false
        if (stateFilter === 'attention' && !['stalled', 'escalated', 'pending_review'].includes(node.state)) return false
        if (stateFilter === 'succeeded' && node.state !== 'succeeded') return false
        if (stateFilter === 'failed' && !['failed', 'cancelled'].includes(node.state)) return false
        if (stateFilter === 'queued' && node.state !== 'queued') return false
      }

      // Check text search query
      if (queryLower) {
        const titleMatch = node.title.toLowerCase().includes(queryLower)
        const idMatch = node.id.toLowerCase().includes(queryLower)
        const harnessMatch = node.harness?.toLowerCase().includes(queryLower) ?? false
        const accountMatch = node.account?.toLowerCase().includes(queryLower) ?? false
        const modelMatch = node.model?.toLowerCase().includes(queryLower) ?? false
        if (!titleMatch && !idMatch && !harnessMatch && !accountMatch && !modelMatch) return false
      }

      return true
    },
    [queryLower, stateFilter],
  )

  const hasFilter = queryLower !== '' || stateFilter !== 'all'
  const matchingNodeIds = useMemo(() => {
    const set = new Set<string>()
    for (const node of layout.nodes) {
      if (isNodeMatch(node)) set.add(node.id)
    }
    return set
  }, [layout.nodes, isNodeMatch])

  // Effective focused node (syncs with selectedRunId, fallback to user focus or first node)
  const effectiveFocusId = selectedRunId && layout.nodeMap.has(selectedRunId)
    ? selectedRunId
    : focusedNodeId && layout.nodeMap.has(focusedNodeId)
      ? focusedNodeId
      : layout.nodes[0]?.id ?? null

  // Active highlighted edges (connected to hovered or focused or selected node)
  const activeNodeId = hoveredNodeId ?? selectedRunId ?? effectiveFocusId
  const highlightedEdgeIds = useMemo(() => {
    const set = new Set<string>()
    if (!activeNodeId) return set
    for (const edge of layout.edges) {
      if (edge.source === activeNodeId || edge.target === activeNodeId) {
        set.add(edge.id)
      }
    }
    return set
  }, [activeNodeId, layout.edges])

  // Message edges (aggregated from model.arrows)
  const messageEdges = useMemo(() => {
    if (!showMessages) return []
    const map = new Map<string, { source: string; target: string; count: number; kinds: Set<string> }>()
    for (const arrow of model.arrows) {
      const key = `${arrow.from}->${arrow.to}`
      const existing = map.get(key)
      if (existing) {
        existing.count++
        existing.kinds.add(arrow.msgKind)
      } else {
        map.set(key, { source: arrow.from, target: arrow.to, count: 1, kinds: new Set([arrow.msgKind]) })
      }
    }

    const res: { id: string; source: string; target: string; count: number; path: string }[] = []
    for (const [key, item] of map) {
      const srcNode = layout.nodeMap.get(item.source)
      const tgtNode = layout.nodeMap.get(item.target)
      if (!srcNode || !tgtNode) continue
      const sx = srcNode.x + srcNode.width / 2
      const sy = srcNode.y + srcNode.height
      const tx = tgtNode.x + tgtNode.width / 2
      const ty = tgtNode.y + tgtNode.height
      const my = Math.max(sy, ty) + 30
      const path = `M ${sx} ${sy} C ${sx} ${my}, ${tx} ${my}, ${tx} ${ty}`
      res.push({ id: `msg-${key}`, source: item.source, target: item.target, count: item.count, path })
    }
    return res
  }, [showMessages, model.arrows, layout.nodeMap])

  // Zoom handlers
  const handleZoomIn = () => setZoom((z) => Math.min(2.0, Number((z * 1.2).toFixed(2))))
  const handleZoomOut = () => setZoom((z) => Math.max(0.4, Number((z / 1.2).toFixed(2))))
  const handleZoomReset = () => setZoom(1)
  const handleFit = useCallback(() => {
    if (!containerRef.current) return
    const rect = containerRef.current.getBoundingClientRect()
    if (rect.width <= 0 || layout.width <= 0) return
    const fitScale = Math.min(1.2, Math.max(0.4, (rect.width - 32) / layout.width))
    setZoom(Number(fitScale.toFixed(2)))
  }, [layout.width])

  // Keyboard navigation across nodes in the graph
  const onNodeKeyDown = (e: KeyboardEvent<HTMLElement>, node: GraphNode) => {
    const { id, colIndex } = node
    let nextId: string | null = null

    switch (e.key) {
      case 'Enter':
      case ' ':
        e.preventDefault()
        if (id !== ORCHESTRATOR_LANE) {
          onSelect(id)
        }
        return

      case 'ArrowRight': {
        e.preventDefault()
        // Try first child, else try next column
        if (node.childrenIds.length > 0) {
          nextId = node.childrenIds[0]
        } else {
          const nextColNodes = layout.nodes.filter((n) => n.colIndex === colIndex + 1)
          if (nextColNodes.length > 0) {
            // Find closest by Y
            nextColNodes.sort((a, b) => Math.abs(a.y - node.y) - Math.abs(b.y - node.y))
            nextId = nextColNodes[0].id
          }
        }
        break
      }

      case 'ArrowLeft': {
        e.preventDefault()
        // Try parent, else try prev column
        if (node.parentId && layout.nodeMap.has(node.parentId)) {
          nextId = node.parentId
        } else {
          const prevColNodes = layout.nodes.filter((n) => n.colIndex === colIndex - 1)
          if (prevColNodes.length > 0) {
            prevColNodes.sort((a, b) => Math.abs(a.y - node.y) - Math.abs(b.y - node.y))
            nextId = prevColNodes[0].id
          }
        }
        break
      }

      case 'ArrowDown': {
        e.preventDefault()
        // Move to next node in same column
        const sameCol = layout.nodes.filter((n) => n.colIndex === colIndex).sort((a, b) => a.rowIndex - b.rowIndex)
        const idx = sameCol.findIndex((n) => n.id === id)
        if (idx !== -1 && idx < sameCol.length - 1) {
          nextId = sameCol[idx + 1].id
        }
        break
      }

      case 'ArrowUp': {
        e.preventDefault()
        // Move to previous node in same column
        const sameCol = layout.nodes.filter((n) => n.colIndex === colIndex).sort((a, b) => a.rowIndex - b.rowIndex)
        const idx = sameCol.findIndex((n) => n.id === id)
        if (idx > 0) {
          nextId = sameCol[idx - 1].id
        }
        break
      }

      case 'Home':
        e.preventDefault()
        nextId = ORCHESTRATOR_LANE
        break

      case 'End':
        e.preventDefault()
        nextId = layout.nodes[layout.nodes.length - 1]?.id ?? null
        break
    }

    if (nextId && layout.nodeMap.has(nextId)) {
      setFocusedNodeId(nextId)
      const nextNode = layout.nodeMap.get(nextId)!
      setLiveAnnouncement(`${nextNode.title}, state: ${nextNode.state}, duration: ${nextNode.formattedDuration}`)
      const el = containerRef.current?.querySelector(`[data-node-id="${nextId}"]`) as HTMLElement | null
      el?.focus()
    }
  }

  return (
    <div
      role="group"
      aria-label="Agent hierarchy graph"
      aria-describedby={graphHelpId}
      className={cn('flex flex-col rounded-md border bg-background overflow-hidden', className)}
      data-testid="agent-graph"
    >
      <p id={graphHelpId} className="sr-only">
        Agent hierarchy graph. Use arrow keys to navigate between agents, Enter to view run details.
      </p>

      {/* Graph Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-b px-3 py-2 bg-muted/20" role="toolbar" aria-label="Graph controls">
        <div className="flex items-center gap-1.5 flex-wrap">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleFit}
            aria-label="Fit graph to window"
            title="Fit to view"
          >
            <Maximize aria-hidden className="size-3.5" />
            <span>Fit</span>
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            onClick={handleZoomOut}
            aria-label="Zoom out"
            title="Zoom out (-)"
          >
            <Minus aria-hidden className="size-3.5" />
          </Button>
          <span className="w-12 text-center text-xs tabular-nums text-muted-foreground font-medium" aria-label={`Current zoom ${Math.round(zoom * 100)}%`}>
            {Math.round(zoom * 100)}%
          </span>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            onClick={handleZoomIn}
            aria-label="Zoom in"
            title="Zoom in (+)"
          >
            <Plus aria-hidden className="size-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={handleZoomReset}
            aria-label="Reset zoom to 100%"
            title="Reset zoom"
          >
            <RotateCcw aria-hidden className="size-3.5" />
          </Button>

          <div className="h-4 w-px bg-border mx-1" aria-hidden />

          {/* Grouping Selector */}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="outline" size="sm" className="gap-1.5" aria-label="Group graph nodes">
                {grouping === 'hierarchy' ? (
                  <GitFork aria-hidden className="size-3.5" />
                ) : grouping === 'status' ? (
                  <Layers aria-hidden className="size-3.5" />
                ) : (
                  <Cpu aria-hidden className="size-3.5" />
                )}
                <span>
                  {grouping === 'hierarchy' ? 'Hierarchy' : grouping === 'status' ? 'By Status' : 'By Harness'}
                </span>
                <ChevronDown aria-hidden className="size-3 text-muted-foreground" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItem onClick={() => setGrouping('hierarchy')}>
                <GitFork aria-hidden className="size-4 mr-2" />
                <span>Hierarchy (DAG)</span>
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => setGrouping('status')}>
                <Layers aria-hidden className="size-4 mr-2" />
                <span>By Status</span>
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => setGrouping('harness')}>
                <Cpu aria-hidden className="size-4 mr-2" />
                <span>By Harness</span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <Button
            type="button"
            variant={showMessages ? 'secondary' : 'outline'}
            size="sm"
            onClick={() => setShowMessages((v) => !v)}
            aria-pressed={showMessages}
            title="Toggle message connection lines"
            className="gap-1"
          >
            <MessageSquare aria-hidden className="size-3.5" />
            <span>Messages</span>
            {model.arrows.length > 0 && (
              <span className="ml-1 rounded-full bg-muted px-1.5 py-0.2 text-[10px] font-semibold tabular-nums">
                {model.arrows.length}
              </span>
            )}
          </Button>
        </div>

        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>{layout.nodes.length} agents</span>
          <span>·</span>
          <span>{layout.edges.length} links</span>
        </div>
      </div>

      {/* Graph Canvas Scroll Area */}
      <div
        ref={containerRef}
        tabIndex={-1}
        className="relative max-h-[min(70vh,760px)] min-h-[460px] overflow-auto p-4 select-none bg-radial from-muted/30 to-background focus:outline-none"
        data-testid="graph-scroll-container"
      >
        <div
          className="relative transition-transform duration-75"
          style={{
            width: layout.width * zoom,
            height: layout.height * zoom,
            minWidth: '100%',
          }}
        >
          <div
            style={{
              transform: `scale(${zoom})`,
              transformOrigin: '0 0',
              width: layout.width,
              height: layout.height,
            }}
          >
            {/* Column Headers */}
            <div className="absolute top-0 left-0 right-0 h-9 pointer-events-none" aria-hidden>
              {layout.columns.map((col) => (
                <div
                  key={col.id}
                  style={{ position: 'absolute', left: col.x, width: col.width }}
                  className="flex items-center justify-between border-b pb-1 text-xs font-semibold text-muted-foreground uppercase tracking-wider"
                >
                  <span className="truncate">{col.title}</span>
                  <span className="rounded-full bg-muted/80 px-1.5 py-0.5 text-[10px] font-medium text-foreground">
                    {col.count}
                  </span>
                </div>
              ))}
            </div>

            {/* SVG Edges Layer */}
            <svg
              className="absolute inset-0 pointer-events-none"
              width={layout.width}
              height={layout.height}
              role="presentation"
              focusable="false"
            >
              <defs>
                <marker
                  id="graph-arrow-head"
                  viewBox="0 0 10 10"
                  refX="8"
                  refY="5"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 2 L 8 5 L 0 8 z" fill="var(--muted-foreground)" />
                </marker>
                <marker
                  id="graph-arrow-head-active"
                  viewBox="0 0 10 10"
                  refX="8"
                  refY="5"
                  markerWidth="7"
                  markerHeight="7"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 2 L 8 5 L 0 8 z" fill="var(--primary)" />
                </marker>
                <marker
                  id="graph-arrow-head-orphan"
                  viewBox="0 0 10 10"
                  refX="8"
                  refY="5"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 2 L 8 5 L 0 8 z" fill="var(--warning, #d97706)" />
                </marker>
                <marker
                  id="graph-arrow-head-msg"
                  viewBox="0 0 10 10"
                  refX="8"
                  refY="5"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 2 L 8 5 L 0 8 z" fill="var(--state-running, #16a34a)" />
                </marker>
              </defs>

              {/* Hierarchy Edges */}
              {layout.edges.map((edge) => {
                const isActive = highlightedEdgeIds.has(edge.id)
                const isOrphan = edge.isOrphan
                return (
                  <path
                    key={edge.id}
                    d={edge.path}
                    fill="none"
                    stroke={
                      isActive
                        ? 'var(--primary)'
                        : isOrphan
                          ? 'var(--warning, #d97706)'
                          : 'var(--border)'
                    }
                    strokeWidth={isActive ? 2.4 : 1.6}
                    strokeDasharray={isOrphan ? '5 4' : undefined}
                    strokeOpacity={isActive ? 1 : 0.75}
                    markerEnd={
                      isActive
                        ? 'url(#graph-arrow-head-active)'
                        : isOrphan
                          ? 'url(#graph-arrow-head-orphan)'
                          : 'url(#graph-arrow-head)'
                    }
                    data-testid={`edge-${edge.id}`}
                  />
                )
              })}

              {/* Message Edges */}
              {messageEdges.map((medge) => (
                <g key={medge.id}>
                  <path
                    d={medge.path}
                    fill="none"
                    stroke="var(--state-running, #16a34a)"
                    strokeWidth={1.4}
                    strokeDasharray="3 3"
                    strokeOpacity={0.65}
                    markerEnd="url(#graph-arrow-head-msg)"
                  />
                </g>
              ))}
            </svg>

            {/* HTML Node Cards */}
            <ul className="absolute inset-0 list-none m-0 p-0 pointer-events-none" aria-label="Agents list">
              {layout.nodes.map((node) => {
                const isSelected = node.id === selectedRunId
                const isFocused = node.id === effectiveFocusId
                const isMatch = !hasFilter || matchingNodeIds.has(node.id)
                const isOrch = node.id === ORCHESTRATOR_LANE
                const stateStyle = STATE_STYLES[node.state]

                return (
                  <li
                    key={node.id}
                    className="absolute list-none m-0 p-0"
                    style={{
                      left: node.x,
                      top: node.y,
                      width: node.width,
                      height: node.height,
                    }}
                  >
                    <button
                      type="button"
                      tabIndex={isFocused ? 0 : -1}
                      data-node-id={node.id}
                      data-testid={`graph-node-${node.id}`}
                      aria-label={`${node.title}, state ${node.state}, duration ${node.formattedDuration}${node.isOrphan ? ', orphan' : ''}`}
                      aria-current={isSelected ? 'true' : undefined}
                      onMouseEnter={() => setHoveredNodeId(node.id)}
                      onMouseLeave={() => setHoveredNodeId(null)}
                      onFocus={() => setFocusedNodeId(node.id)}
                      onClick={() => {
                        setFocusedNodeId(node.id)
                        if (!isOrch) onSelect(node.id)
                      }}
                      onKeyDown={(e) => onNodeKeyDown(e, node)}
                      className={cn(
                        'pointer-events-auto text-left w-full h-full group rounded-lg border bg-card p-2.5 shadow-xs transition-all outline-none select-none flex flex-col justify-between',
                        !isOrch && 'cursor-pointer hover:border-primary/60 hover:shadow-md',
                        isSelected && 'ring-2 ring-primary border-primary bg-accent/25 shadow-md',
                        isFocused && !isSelected && 'ring-2 ring-ring/80 border-ring',
                        !isMatch && 'opacity-25 grayscale-[50%]',
                        node.isOrphan && 'border-amber-500/60 dark:border-amber-500/40',
                      )}
                    >
                    {/* Header Row: State & Depth */}
                    <div className="flex items-center justify-between gap-1.5">
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span
                          className="size-2.5 shrink-0 rounded-full ring-1 ring-border"
                          style={{ background: stateFill(node.state) }}
                          aria-hidden
                        />
                        <span className="text-[11px] font-semibold truncate capitalize" style={{ color: stateFill(node.state) }}>
                          {stateStyle?.label ?? node.state}
                        </span>
                        {stateStyle?.endGlyph === 'check' && (
                          <Check aria-hidden className="size-3 text-emerald-600 dark:text-emerald-400" />
                        )}
                        {stateStyle?.endGlyph === 'x' && (
                          <X aria-hidden className="size-3 text-rose-600 dark:text-rose-400" />
                        )}
                      </div>

                      <div className="flex items-center gap-1 shrink-0">
                        {node.isOrphan && (
                          <span
                            title={`Orphan: parent run "${node.missingParentId}" was not found`}
                            className="inline-flex items-center gap-0.5 rounded px-1 py-0.2 text-[9px] font-semibold bg-amber-500/15 text-amber-600 dark:text-amber-400 border border-amber-500/30"
                          >
                            <AlertTriangle className="size-2.5" />
                            Orphan
                          </span>
                        )}
                        <span className="rounded bg-muted px-1.5 py-0.2 text-[10px] font-medium text-muted-foreground">
                          {isOrch ? 'Root' : node.depth === 0 ? 'Depth 0' : `Depth ${node.depth}`}
                        </span>
                      </div>
                    </div>

                    {/* Middle: Title & Identifier */}
                    <div className="min-w-0 my-0.5">
                      <div className="truncate text-xs font-semibold leading-tight text-foreground group-hover:text-primary transition-colors" title={node.title}>
                        {node.title}
                      </div>
                      {!isOrch && (
                        <div className="truncate text-[10px] font-mono text-muted-foreground" title={node.id}>
                          {node.id}
                        </div>
                      )}
                    </div>

                    {/* Footer Row: Metadata (duration, harness, counts) */}
                    <div className="flex items-center justify-between gap-1 text-[10px] text-muted-foreground border-t pt-1">
                      <div className="flex items-center gap-1 shrink-0">
                        <Clock aria-hidden className="size-2.5" />
                        <span className="tabular-nums font-medium">{node.formattedDuration}</span>
                      </div>

                      <div className="flex items-center gap-1.5 shrink-0">
                        {node.harness && (
                          <span className="rounded bg-secondary/80 px-1 py-0.2 font-medium text-secondary-foreground">
                            {node.harness}
                          </span>
                        )}
                        {node.toolCount > 0 && (
                          <span className="flex items-center gap-0.5" title={`${node.toolCount} tool calls`}>
                            <Wrench aria-hidden className="size-2.5" />
                            <span className="tabular-nums">{node.toolCount}</span>
                          </span>
                        )}
                        {node.messageCount > 0 && (
                          <span className="flex items-center gap-0.5" title={`${node.messageCount} messages`}>
                            <MessageSquare aria-hidden className="size-2.5" />
                            <span className="tabular-nums">{node.messageCount}</span>
                          </span>
                        )}
                      </div>
                    </div>
                  </button>
                </li>
              )
            })}
          </ul>
          </div>
        </div>
      </div>

      {/* Screen reader live announcement */}
      <div aria-live="polite" role="status" className="sr-only" data-testid="graph-live-region">
        {liveAnnouncement}
      </div>
    </div>
  )
}
