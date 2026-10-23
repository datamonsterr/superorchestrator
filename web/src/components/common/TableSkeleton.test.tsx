import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { TableSkeleton } from './TableSkeleton'

it('renders the requested shape as a busy status region', () => {
  render(<TableSkeleton rows={3} columns={5} />)
  expect(screen.getByRole('status', { busy: true })).toBeInTheDocument()
  expect(screen.getAllByTestId('skeleton-row')).toHaveLength(3)
  expect(screen.getAllByRole('columnheader')).toHaveLength(5)
})

it('gives every header cell accessible text', () => {
  render(<TableSkeleton rows={1} columns={3} />)
  for (const th of screen.getAllByRole('columnheader')) {
    expect(th.textContent?.trim()).not.toBe('')
  }
})
