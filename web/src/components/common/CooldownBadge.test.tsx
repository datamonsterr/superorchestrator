import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { CooldownBadge } from './CooldownBadge'

const now = Date.parse('2026-11-04T08:00:00Z')

describe('CooldownBadge', () => {
  it('shows the end time and the time left', () => {
    render(<CooldownBadge now={now} flag={{ pool: 'claude', until: '2026-11-04T10:10:00Z', reason: 'quota' }} />)
    const badge = screen.getByText((_, el) => el?.getAttribute('data-cooldown') === 'claude')
    expect(badge).toHaveTextContent('(in 2h 10m)')
    expect(badge).toHaveAttribute('title', 'quota')
    expect(badge.querySelector('time')).toHaveAttribute('datetime', '2026-11-04T10:10:00.000Z')
  })

  it('names no pool for an account-wide cooldown and handles a passed end', () => {
    render(<CooldownBadge now={now} flag={{ until: '2026-11-04T07:59:00Z' }} />)
    expect(screen.getByText((_, el) => el?.getAttribute('data-cooldown') === 'all')).toHaveTextContent('(ending now)')
  })
})
