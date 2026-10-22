import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { stringifyJson } from '@/lib/format'
import { JsonView } from './JsonView'

describe('JsonView', () => {
  it('is collapsed by default and expands on click', async () => {
    render(<JsonView value={{ a: 1 }} label="Result" />)
    expect(screen.queryByText(/"a": 1/)).not.toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: /Result/ }))
    expect(screen.getByText(/"a": 1/)).toBeInTheDocument()
  })
  it('renders untrusted markup as text', () => {
    render(<JsonView value={{ html: '<img src=x onerror=alert(1)>' }} defaultOpen />)
    expect(document.querySelector('img')).toBeNull()
    expect(screen.getByText(/<img src=x/)).toBeInTheDocument()
  })
  it('survives circular and undefined values', () => {
    const o: Record<string, unknown> = {}
    o.self = o
    expect(stringifyJson(o)).toBe('[unserialisable value]')
    expect(stringifyJson(undefined)).toBe('undefined')
  })
})
