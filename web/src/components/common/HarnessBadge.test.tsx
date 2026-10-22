import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { HarnessBadge } from './HarnessBadge'

it('shows the harness name', () => {
  render(<HarnessBadge harness="codex" />)
  expect(screen.getByText('Codex')).toHaveAttribute('data-harness', 'codex')
})
