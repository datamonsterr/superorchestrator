import { render, screen, within } from '@testing-library/react'
import { expect, it } from 'vitest'
import { ProbabilityBars } from './ProbabilityBars'

it('sorts descending, prints percents and highlights the selection', () => {
  render(<ProbabilityBars probabilities={{ a: 0.21, b: 0.62, c: 0.17 }} selected="b" />)
  const items = screen.getAllByRole('listitem')
  expect(items.map((li) => li.textContent)).toEqual(['b62%selected', 'a21%', 'c17%'])
  expect(items[0]).toHaveAttribute('data-selected', 'true')
  expect(within(items[1]).getByText('21%')).toBeInTheDocument()
  expect(items[1]).not.toHaveAttribute('data-selected')
})

it('handles an empty map', () => {
  render(<ProbabilityBars probabilities={{}} />)
  expect(screen.getByText('No probabilities')).toBeInTheDocument()
})
