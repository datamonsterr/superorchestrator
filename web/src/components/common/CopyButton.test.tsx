import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import { CopyButton } from './CopyButton'

it('writes to the clipboard and shows confirmation', async () => {
  const user = userEvent.setup()
  render(<CopyButton value="hello" label="Copy it" />)
  await user.click(screen.getByRole('button', { name: 'Copy it' }))
  expect(await navigator.clipboard.readText()).toBe('hello')
  expect(await screen.findByText('Copied')).toBeInTheDocument()
})

it('stays quiet when the clipboard is unavailable', async () => {
  const user = userEvent.setup()
  render(<CopyButton value="x" />)
  vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('denied'))
  await user.click(screen.getByRole('button'))
  expect(screen.queryByText('Copied')).not.toBeInTheDocument()
})
