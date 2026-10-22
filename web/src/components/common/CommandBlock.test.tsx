import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it } from 'vitest'
import { CommandBlock } from './CommandBlock'

it('shows the command and copies it with the cwd prefix', async () => {
  const user = userEvent.setup()
  render(<CommandBlock command="codex login" cwd="/home/me/.codex-acc1" />)
  expect(screen.getByText('cd /home/me/.codex-acc1 && codex login')).toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Copy command' }))
  expect(await navigator.clipboard.readText()).toBe('cd /home/me/.codex-acc1 && codex login')
})

it('shows the bare command without a cwd', () => {
  render(<CommandBlock command="superorchestrator install" />)
  expect(screen.getByText('superorchestrator install')).toBeInTheDocument()
})
