import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { runs } from '@/api/msw/fixtures'
import { renderWithProviders } from '@/test/render'
import { LiveRuns } from './LiveRuns'

describe('LiveRuns', () => {
  it('links each run to its session timeline using run.session_id', () => {
    renderWithProviders(<LiveRuns runs={[{ ...runs[0], session_id: 'sess-9' }]} />)
    expect(screen.getByRole('link', { name: 'refactor-api' })).toHaveAttribute('href', '/sessions/sess-9/runs/run-1')
  })
  it('shows a plain label for a run without a session id', () => {
    renderWithProviders(<LiveRuns runs={[{ ...runs[0], session_id: '' }]} />)
    expect(screen.queryByRole('link')).toBeNull()
    expect(screen.getByText('refactor-api')).toBeInTheDocument()
  })
})
