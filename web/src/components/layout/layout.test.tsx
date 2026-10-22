import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes, useLocation } from 'react-router'
import { beforeEach, describe, expect, it } from 'vitest'
import { StreamContext } from '@/api/stream'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { THEME_KEY } from '@/lib/theme'
import { capLevel } from '@/lib/cap'
import { CapMeter, CapMeterView } from './CapMeter'
import { CommandPalette } from './CommandPalette'
import { SupervisorPill } from './SupervisorPill'
import { ThemeProvider } from './ThemeProvider'
import { ThemeToggle } from './ThemeToggle'

describe('SupervisorPill', () => {
  it('shows up with uptime when the stream is open', async () => {
    renderWithProviders(
      <StreamContext.Provider value={{ status: 'open', polling: false, bootId: 'b' }}>
        <SupervisorPill />
      </StreamContext.Provider>,
    )
    expect(await screen.findByText(/up 3h 0m/)).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveAttribute('data-health', 'up')
  })

  it('is amber while the stream is down', async () => {
    renderWithProviders(
      <StreamContext.Provider value={{ status: 'reconnecting', polling: true, bootId: null }}>
        <SupervisorPill />
      </StreamContext.Provider>,
    )
    await waitFor(() => expect(screen.getByRole('status')).toHaveAttribute('data-health', 'reconnecting'))
    expect(screen.getByText('reconnecting')).toBeInTheDocument()
  })

  it('is red when /status fails', async () => {
    server.use(http.get('*/api/v1/status', () => HttpResponse.json({ error: { code: -32004, message: 'down' } }, { status: 503 })))
    renderWithProviders(<SupervisorPill />)
    await waitFor(() => expect(screen.getByRole('status')).toHaveAttribute('data-health', 'unreachable'), { timeout: 4000 })
    expect(screen.getByText('unreachable')).toBeInTheDocument()
  })
})

describe('CapMeter', () => {
  it.each([
    [0, 8, 'ok'],
    [6, 8, 'ok'],
    [7, 8, 'warn'],
    [8, 8, 'full'],
    [9, 8, 'full'],
    [1, 0, 'ok'],
  ] as const)('%s of %s is %s', (a, c, level) => expect(capLevel(a, c)).toBe(level))

  it('renders from /status', async () => {
    renderWithProviders(<CapMeter />)
    expect(await screen.findByText('3/8')).toBeInTheDocument()
    expect(screen.getByRole('progressbar', { name: 'Global cap usage' })).toHaveAttribute('aria-valuenow', '3')
  })

  it('flags a full cap', () => {
    const { container } = renderWithProviders(<CapMeterView active={8} cap={8} />)
    expect(container.firstChild).toHaveAttribute('data-level', 'full')
  })
})

describe('ThemeToggle', () => {
  beforeEach(() => {
    localStorage.clear()
    document.documentElement.classList.remove('dark')
  })

  it('applies dark, persists it, and can go back to light', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <ThemeProvider>
        <ThemeToggle />
      </ThemeProvider>,
    )
    await user.click(screen.getByRole('button', { name: /Theme/ }))
    await user.click(await screen.findByRole('menuitemradio', { name: /Dark/ }))
    expect(document.documentElement).toHaveClass('dark')
    expect(localStorage.getItem(THEME_KEY)).toBe('dark')
    await user.click(screen.getByRole('button', { name: /Theme: dark/ }))
    await user.click(await screen.findByRole('menuitemradio', { name: /Light/ }))
    expect(document.documentElement).not.toHaveClass('dark')
  })

  it('reads the stored theme on mount', () => {
    localStorage.setItem(THEME_KEY, 'dark')
    renderWithProviders(
      <ThemeProvider>
        <ThemeToggle />
      </ThemeProvider>,
    )
    expect(document.documentElement).toHaveClass('dark')
  })
})

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>
}

describe('CommandPalette', () => {
  it('opens with ctrl+K and jumps to a page', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <>
        <CommandPalette />
        <Routes>
          <Route path="*" element={<Where />} />
        </Routes>
      </>,
    )
    await user.keyboard('{Control>}k{/Control}')
    expect(await screen.findByPlaceholderText(/Jump to a page/)).toBeInTheDocument()
    await user.click(await screen.findByRole('option', { name: /Accounts/ }))
    expect(screen.getByTestId('where')).toHaveTextContent('/accounts')
  })

  it('lists sessions from the API and opens one', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <>
        <CommandPalette />
        <Routes>
          <Route path="*" element={<Where />} />
        </Routes>
      </>,
    )
    await user.click(screen.getByRole('button', { name: 'Open command palette' }))
    await user.type(await screen.findByPlaceholderText(/Jump to a page/), 'other')
    await user.click(await screen.findByRole('option', { name: /codex-acc1/ }))
    expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-2')
  })

  it('opens a run id via the API', async () => {
    const user = userEvent.setup()
    server.use(
      http.get('*/api/v1/runs/run-1', () =>
        HttpResponse.json({
          run_id: 'run-1', state: 'running', depth: 0, usage: { input_tokens: 0, output_tokens: 0, cost: 0 },
          recent_events: [{ id: 'e', session_id: 'sess-1', run_id: 'run-1', ts: '2026-01-01T00:00:00Z', type: 'run_started', data: {} }],
        }),
      ),
    )
    renderWithProviders(
      <>
        <CommandPalette />
        <Routes>
          <Route path="*" element={<Where />} />
        </Routes>
      </>,
    )
    await user.click(screen.getByRole('button', { name: 'Open command palette' }))
    await user.type(await screen.findByPlaceholderText(/Jump to a page/), 'run-1')
    await user.click(await screen.findByRole('option', { name: /Open run/ }))
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent('/sessions/sess-1/runs/run-1'))
  })
})
