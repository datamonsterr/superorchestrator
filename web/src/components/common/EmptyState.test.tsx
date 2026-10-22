import { render, screen } from '@testing-library/react'
import { Inbox } from 'lucide-react'
import { expect, it } from 'vitest'
import { EmptyState } from './EmptyState'

it('renders title, description and action', () => {
  render(<EmptyState icon={Inbox} title="No sessions yet" description="Connect an orchestrator." action={<button>Add</button>} />)
  expect(screen.getByText('No sessions yet')).toBeInTheDocument()
  expect(screen.getByText('Connect an orchestrator.')).toBeInTheDocument()
  expect(screen.getByRole('button', { name: 'Add' })).toBeInTheDocument()
})
