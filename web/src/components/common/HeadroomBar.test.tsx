import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { headroomLevel } from '@/lib/headroom'
import { HeadroomBar } from './HeadroomBar'

describe('headroomLevel', () => {
  it.each([
    [72, 'ok', 'ok'],
    [31, 'ok', 'ok'],
    [30, 'low', 'warn'],
    [10, 'low', 'warn'],
    [9, 'low', 'critical'],
    [50, 'exhausted', 'critical'],
    [null, 'unknown', 'unknown'],
    [undefined, undefined, 'unknown'],
  ] as const)('%s / %s -> %s', (pct, status, level) => {
    expect(headroomLevel(pct, status)).toBe(level)
  })
})

describe('HeadroomBar', () => {
  it('shows the percent as text and exposes progressbar values', () => {
    render(<HeadroomBar pct={72} label="5h" />)
    expect(screen.getByText('72%')).toBeInTheDocument()
    const bar = screen.getByRole('progressbar', { name: '5h headroom' })
    expect(bar).toHaveAttribute('aria-valuenow', '72')
    expect(bar).toHaveAttribute('data-level', 'ok')
  })

  it('renders unknown with text, no value and no fill', () => {
    render(<HeadroomBar pct={null} status="unknown" />)
    const bar = screen.getByRole('progressbar')
    expect(screen.getByText('unknown')).toBeInTheDocument()
    expect(bar).not.toHaveAttribute('aria-valuenow')
    expect(bar.firstChild).toBeNull()
  })

  it('colours critical when exhausted regardless of pct', () => {
    render(<HeadroomBar pct={40} status="exhausted" />)
    expect(screen.getByRole('progressbar')).toHaveAttribute('data-level', 'critical')
  })
})
