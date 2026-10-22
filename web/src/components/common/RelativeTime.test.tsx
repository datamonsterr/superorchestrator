import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { formatRelative } from '@/lib/format'
import { RelativeTime } from './RelativeTime'

const NOW = Date.UTC(2026, 0, 1, 12, 0, 0)

describe('RelativeTime', () => {
  it('renders a <time> with a relative label', () => {
    render(<RelativeTime value={NOW - 5 * 60_000} now={NOW} />)
    const t = screen.getByText('5 minutes ago')
    expect(t.tagName).toBe('TIME')
    expect(t).toHaveAttribute('datetime', new Date(NOW - 5 * 60_000).toISOString())
  })
  it('handles missing and invalid values', () => {
    const { rerender } = render(<RelativeTime value={null} now={NOW} />)
    expect(screen.getByText('-')).toBeInTheDocument()
    rerender(<RelativeTime value="nope" now={NOW} />)
    expect(screen.getByText('unknown')).toBeInTheDocument()
  })
})

describe('formatRelative', () => {
  it.each([
    [NOW - 2000, 'just now'],
    [NOW - 3_600_000 * 3, '3 hours ago'],
    [NOW + 2 * 86_400_000, 'in 2 days'],
    [NOW - 86_400_000, 'yesterday'],
  ])('%s -> %s', (ts, out) => expect(formatRelative(ts, NOW)).toBe(out))
})
