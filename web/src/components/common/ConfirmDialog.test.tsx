import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import { ConfirmDialog } from './ConfirmDialog'

it('confirms and cancels', async () => {
  const user = userEvent.setup()
  const onConfirm = vi.fn()
  const onOpenChange = vi.fn()
  render(<ConfirmDialog open onOpenChange={onOpenChange} title="Cancel run?" description="Stops the subtree." confirmLabel="Cancel run" onConfirm={onConfirm} />)
  expect(screen.getByRole('alertdialog', { name: 'Cancel run?' })).toBeInTheDocument()
  await user.click(screen.getByRole('button', { name: 'Cancel run' }))
  expect(onConfirm).toHaveBeenCalledOnce()
  await user.click(screen.getByRole('button', { name: 'Cancel' }))
  expect(onOpenChange).toHaveBeenCalledWith(false)
})

it('requires typing the exact text before confirming', async () => {
  const user = userEvent.setup()
  const onConfirm = vi.fn()
  render(<ConfirmDialog open onOpenChange={() => {}} title="Remove" requireText="codex-acc1" confirmLabel="Remove" destructive onConfirm={onConfirm} />)
  const confirm = screen.getByRole('button', { name: 'Remove' })
  expect(confirm).toBeDisabled()
  await user.type(screen.getByRole('textbox'), 'codex-acc')
  expect(confirm).toBeDisabled()
  await user.type(screen.getByRole('textbox'), '1')
  expect(confirm).toBeEnabled()
  await user.click(confirm)
  expect(onConfirm).toHaveBeenCalledOnce()
})
