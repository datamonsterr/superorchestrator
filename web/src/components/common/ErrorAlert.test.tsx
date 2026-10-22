import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import { ErrorAlert } from './ErrorAlert'

it('shows message and API code, and retries', async () => {
  const onRetry = vi.fn()
  render(<ErrorAlert error={new ApiError(409, -32002, 'cap exceeded')} onRetry={onRetry} />)
  expect(screen.getByRole('alert')).toHaveTextContent('cap exceeded')
  expect(screen.getByText('code -32002')).toBeInTheDocument()
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
  expect(onRetry).toHaveBeenCalledOnce()
})

it('copes with non-Error values and no retry', () => {
  render(<ErrorAlert error="plain" title="Oops" />)
  expect(screen.getByText('Oops')).toBeInTheDocument()
  expect(screen.getByText('plain')).toBeInTheDocument()
  expect(screen.queryByRole('button')).not.toBeInTheDocument()
})
