import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { HarnessBadge } from './HarnessBadge'

it('shows the harness name', () => {
  render(<HarnessBadge harness="codex" />)
  expect(screen.getByText('Codex')).toHaveAttribute('data-harness', 'codex')
})

it.each([
  ['copilot', 'Copilot'],
  ['gemini', 'Gemini'],
  ['kimi', 'Kimi'],
  ['cursor', 'Cursor'],
  ['qwen', 'Qwen'],
] as const)('shows %s label and harness attribute', (harness, label) => {
  render(<HarnessBadge harness={harness} />)
  expect(screen.getByText(label)).toHaveAttribute('data-harness', harness)
})
