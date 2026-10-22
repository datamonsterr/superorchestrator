import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import type { RunState } from '@/api/types'
import { StateBadge } from './StateBadge'

const STATES: [RunState, string][] = [
  ['queued', 'Queued'], ['starting', 'Starting'], ['running', 'Running'], ['waiting', 'Waiting'],
  ['stalled', 'Stalled'], ['escalated', 'Escalated'], ['pending_review', 'Pending review'],
  ['succeeded', 'Succeeded'], ['failed', 'Failed'], ['cancelled', 'Cancelled'],
]

it.each(STATES)('renders the text label for %s', (state, label) => {
  render(<StateBadge state={state} />)
  expect(screen.getByText(label)).toBeInTheDocument()
})
