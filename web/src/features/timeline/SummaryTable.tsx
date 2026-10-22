import { memo } from 'react'
import { MARKER_STYLES, STATE_STYLES } from './colors'
import { accountAt, laneTitle } from './layout'
import type { RunLane, TimelineModel } from './model'
import { formatClock } from '@/lib/time'

/** Rows of one lane; memoised on lane identity so a tick redraws only the lanes that changed. */
const LaneRows = memo(function LaneRows({ lane }: { lane: RunLane }) {
  const title = laneTitle(lane)
  return (
    <>
      {lane.segments.map((s, i) => (
        <tr key={`s${i}`}>
          <th scope="row">{title}</th>
          <td>State</td>
          <td>{STATE_STYLES[s.state].label}</td>
          <td>{formatClock(s.start, true)}</td>
          <td>{s.open ? 'ongoing' : formatClock(s.end, true)}</td>
          <td>{accountAt(lane, s.start) ?? ''}</td>
        </tr>
      ))}
      {lane.markers.map((m, i) => (
        <tr key={`m${i}`}>
          <th scope="row">{title}</th>
          <td>{MARKER_STYLES[m.kind].label}</td>
          <td>{m.label}</td>
          <td>{formatClock(m.at, true)}</td>
          <td />
          <td />
        </tr>
      ))}
    </>
  )
})

/** Visually hidden table with the same data as the plot (design 4.6). */
export function SummaryTable({ model }: { model: TimelineModel }) {
  return (
    <table className="sr-only" data-testid="timeline-summary">
      <caption>Timeline summary: one row per state segment and marker</caption>
      <thead>
        <tr>
          <th scope="col">Run</th>
          <th scope="col">Kind</th>
          <th scope="col">Detail</th>
          <th scope="col">Start</th>
          <th scope="col">End</th>
          <th scope="col">Account</th>
        </tr>
      </thead>
      <tbody>
        {model.lanes.map((lane) => (
          <LaneRows key={lane.runId} lane={lane} />
        ))}
      </tbody>
    </table>
  )
}
