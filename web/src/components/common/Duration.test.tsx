import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { formatDuration, formatMoney, formatPercent, formatTokens } from '@/lib/format'
import { Duration } from './Duration'

describe('Duration', () => {
  it('formats ms', () => {
    render(<Duration ms={65_000} />)
    expect(screen.getByText('1m 5s')).toBeInTheDocument()
  })
  it('computes a range and uses now for open ranges', () => {
    const start = '2026-01-01T12:00:00Z'
    const { rerender } = render(<Duration start={start} end="2026-01-01T13:01:00Z" />)
    expect(screen.getByText('1h 1m')).toBeInTheDocument()
    rerender(<Duration start={start} now={Date.parse(start) + 30_000} />)
    expect(screen.getByText('30s')).toBeInTheDocument()
  })
  it('renders a dash with nothing to show', () => {
    render(<Duration />)
    expect(screen.getByText('-')).toBeInTheDocument()
  })
})

describe('format helpers', () => {
  it('formatDuration', () => {
    expect(formatDuration(0)).toBe('0s')
    expect(formatDuration(-5)).toBe('0s')
    expect(formatDuration(90_061_000)).toBe('1d 1h')
  })
  it('formatTokens', () => {
    expect(formatTokens(999)).toBe('999')
    expect(formatTokens(1500)).toBe('1.5k')
    expect(formatTokens(2_000_000)).toBe('2M')
    expect(formatTokens(250_000)).toBe('250k')
  })
  it('formatMoney and formatPercent', () => {
    expect(formatMoney(3.214)).toBe('$3.21')
    expect(formatMoney(0.0004)).toBe('$0.0004')
    expect(formatPercent(71.6)).toBe('72%')
    expect(formatPercent(null)).toBe('unknown')
  })
})
