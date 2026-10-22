import { memo, type KeyboardEvent } from 'react'
import { STATE_STYLES, stateFill } from './colors'
import { accountAt, laneCurrentState, laneTitle, LANE_H } from './layout'
import { ORCHESTRATOR_LANE, type RunLane } from './model'
import { cn } from '@/lib/utils'

interface Props {
  lane: RunLane
  y: number
  selected: boolean
  onSelect: (runId: string) => void
  onKeyDown?: (e: KeyboardEvent<HTMLLIElement>) => void
}

/** Sticky-left lane label (HTML): indent by depth, state dot, name, state and account. Collapses to initials below `sm`. */
export const LaneLabel = memo(function LaneLabel({ lane, y, selected, onSelect, onKeyDown }: Props) {
  const title = laneTitle(lane)
  const state = laneCurrentState(lane)
  const acct = accountAt(lane, lane.segments[lane.segments.length - 1]?.start ?? 0)
  const sub = lane.runId === ORCHESTRATOR_LANE ? 'Messages and joins' : [state ? STATE_STYLES[state].label : null, acct].filter(Boolean).join(' · ')
  const initials = title.replace(/[^A-Za-z0-9]+/g, ' ').trim().split(' ').map((w) => w[0]).join('').slice(0, 2).toUpperCase() || '?'
  return (
    <li
      role="listitem"
      tabIndex={0}
      data-lane={lane.runId}
      data-testid={`label-${lane.runId}`}
      aria-label={`${title}${sub ? `, ${sub}` : ''}`}
      aria-current={selected ? 'true' : undefined}
      title={`${title}${sub ? ` (${sub})` : ''}`}
      onClick={() => onSelect(lane.runId)}
      onKeyDown={onKeyDown}
      style={{ position: 'absolute', top: y, left: 0, right: 0, height: LANE_H }}
      className={cn(
        'flex cursor-pointer items-center gap-1.5 border-b px-1.5 outline-none select-none hover:bg-muted/60 focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-ring sm:px-2',
        selected && 'bg-accent',
      )}
    >
      <span aria-hidden style={{ width: lane.depth * 12 }} className="hidden shrink-0 sm:block" />
      {lane.depth > 0 && (
        <span aria-hidden className="hidden shrink-0 text-muted-foreground sm:block">
          └
        </span>
      )}
      <span aria-hidden className="size-2.5 shrink-0 rounded-full border border-background ring-1 ring-border" style={{ background: state ? stateFill(state) : 'var(--muted)' }} />
      <span aria-hidden className="text-[10px] font-semibold sm:hidden">
        {initials}
      </span>
      <span className="hidden min-w-0 flex-1 sm:block">
        <span className="block truncate text-xs leading-4 font-medium">{title}</span>
        <span className="block truncate text-[10px] leading-3 text-muted-foreground">{sub}</span>
      </span>
    </li>
  )
})
