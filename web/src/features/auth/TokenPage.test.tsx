import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { Route, Routes } from 'react-router'
import { expect, it } from 'vitest'
import { server } from '@/test/server'
import { renderWithProviders } from '@/test/render'
import { TokenPage } from './TokenPage'

const ui = (
  <Routes>
    <Route path="/token" element={<TokenPage />} />
    <Route path="/dashboard" element={<h1>Dashboard</h1>} />
  </Routes>
)

it('posts the token then redirects to the dashboard', async () => {
  let body: unknown
  server.use(
    http.post('*/api/v1/auth', async ({ request }) => {
      body = await request.json()
      return new HttpResponse(null, { status: 204 })
    }),
  )
  const user = userEvent.setup()
  renderWithProviders(ui, { route: '/token' })
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled()
  const input = screen.getByLabelText('Access token')
  expect(input).toHaveAttribute('type', 'password')
  await user.type(input, 's3cret')
  await user.click(screen.getByRole('button', { name: 'Continue' }))
  expect(await screen.findByRole('heading', { name: 'Dashboard' })).toBeInTheDocument()
  expect(body).toEqual({ token: 's3cret' })
})

it('shows the API error and stays on the page', async () => {
  server.use(http.post('*/api/v1/auth', () => HttpResponse.json({ error: { code: -32000, message: 'invalid token' } }, { status: 403 })))
  const user = userEvent.setup()
  renderWithProviders(ui, { route: '/token' })
  await user.type(screen.getByLabelText('Access token'), 'bad')
  await user.click(screen.getByRole('button', { name: 'Continue' }))
  expect(await screen.findByRole('alert')).toHaveTextContent('invalid token')
  expect(screen.getByLabelText('Access token')).toBeInTheDocument()
})


it('exchanges the CLI fragment once and removes it before posting', async () => {
  let body: unknown
  let requests = 0
  server.use(http.post('*/api/v1/auth', async ({ request }) => {
    requests++
    expect(window.location.hash).toBe('')
    body = await request.json()
    return new HttpResponse(null, { status: 204 })
  }))
  window.history.replaceState(null, '', '/token#token=secret%20%23%26%2B%25')
  renderWithProviders(ui, { route: '/token#token=secret%20%23%26%2B%25' })
  expect(await screen.findByRole('heading', { name: 'Dashboard' })).toBeInTheDocument()
  expect(body).toEqual({ token: 'secret #&+%' })
  expect(requests).toBe(1)
  expect(window.location.hash).toBe('')
  window.history.replaceState(null, '', '/')
})

it('clears a rejected CLI fragment and allows a manual retry', async () => {
  server.use(http.post('*/api/v1/auth', () => HttpResponse.json({ error: { code: -32000, message: 'invalid token' } }, { status: 401 })))
  window.history.replaceState(null, '', '/token#token=bad')
  renderWithProviders(ui, { route: '/token#token=bad' })
  expect(await screen.findByRole('alert')).toHaveTextContent('invalid token')
  expect(window.location.hash).toBe('')
  expect(screen.getByLabelText('Access token')).toHaveValue('')
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled()
  window.history.replaceState(null, '', '/')
})
