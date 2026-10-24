import { expect, test as base, type Page } from '@playwright/test'
import { execFileSync } from 'node:child_process'

/**
 * Runs against a real supervisor with seeded data (see global-setup.ts), so it catches what the MSW mocks hide:
 * a response shape that differs from the fixtures, and styling that only breaks in the built app under the
 * server's CSP.
 */
const test = base
// Set by global-setup.ts before any worker loads this file.
test.use({ baseURL: process.env.E2E_REAL_URL })
// Exercise the same fragment-to-cookie exchange used by the local CLI, rather than
// bypassing the browser flow with an Authorization header.
function freshLoginUrl(): string {
  const result = execFileSync(process.env.E2E_REAL_CLI!, [
    '--config-dir', process.env.E2E_REAL_CONFIG_DIR!, '--json', 'ui', '--no-open',
  ], { encoding: 'utf8', env: {
    ...process.env, HOME: process.env.E2E_REAL_HOME!,
    SUPERORCHESTRATOR_SOCKET: process.env.E2E_REAL_SOCKET!,
  } })
  return JSON.parse(result).url
}

test.beforeEach(async ({ page, request }) => {
  const loginUrl = freshLoginUrl()
  await page.goto(loginUrl)
  await expect(page.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeVisible()
  await expect(page).toHaveURL(`${process.env.E2E_REAL_URL}/dashboard`)
  const token = new URLSearchParams(new URL(freshLoginUrl()).hash.slice(1)).get('token')
  const response = await request.post('/api/v1/token', { data: { token } })
  expect(response.status()).toBe(204)
})

const session = () => process.env.E2E_REAL_SESSION!
const runs = () => JSON.parse(process.env.E2E_REAL_RUNS!) as Record<string, string>

function watchErrors(page: Page) {
  const errors: string[] = []
  page.on('console', (m) => m.type() === 'error' && errors.push(`console: ${m.text()}`))
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`))
  page.on('response', (r) => r.url().includes('/api/') && r.status() >= 400 && errors.push(`${r.status()} ${r.url()}`))
  return errors
}

const ROUTES: [string, string][] = [
  ['/dashboard', 'Dashboard'],
  ['/sessions', 'Sessions'],
  ['/accounts', 'Accounts'],
  ['/routing', 'Levels and routing'],
  ['/agents', 'Agents'],
  ['/workflows', 'Workflows'],
  ['/settings', 'Settings'],
]

for (const [route, heading] of ROUTES) {
  test(`${route} renders without console errors or an error boundary`, async ({ page }) => {
    const errors = watchErrors(page)
    await page.goto(route)
    await expect(page.getByRole('heading', { level: 1, name: heading })).toBeVisible()
    await page.waitForLoadState('networkidle')
    await expect(page.getByText('This page failed to load')).toHaveCount(0)
    await expect(page.getByRole('alert')).toHaveCount(0)
    expect(errors).toEqual([])
  })
}

test('the routing page shows the seeded levels and their model refs', async ({ page }) => {
  const errors = watchErrors(page)
  await page.goto('/routing')
  for (const name of ['trivial', 'simple', 'moderate', 'complex', 'expert']) {
    await expect(page.getByRole('region', { name: `Level ${name}` })).toBeVisible()
  }
  await expect(page.getByRole('region', { name: 'Level expert' }).getByText('codex/gpt-6.1-sol/high', { exact: true })).toBeVisible()
  await page.waitForLoadState('networkidle')
  expect(errors).toEqual([])
})

test('the session timeline draws lanes with real state colours', async ({ page }) => {
  const errors = watchErrors(page)
  await page.goto(`/sessions/${session()}`)
  const lanes = page.getByRole('list', { name: 'Runs' }).getByRole('listitem')
  await expect(lanes).toHaveCount(5) // orchestrator lane and four runs
  await expect(page.getByText('This page failed to load')).toHaveCount(0)

  const black = ['rgb(0, 0, 0)', 'rgba(0, 0, 0, 0)']
  // Legend swatches: one per state, none black.
  const swatches = page.getByRole('list', { name: 'Run states' }).locator('svg > rect:first-child')
  await expect(swatches).toHaveCount(10)
  const legend = await swatches.evaluateAll((els) => els.map((e) => getComputedStyle(e).fill))
  for (const fill of legend) expect(black, `legend fill ${fill}`).not.toContain(fill)
  expect(new Set(legend).size).toBe(10)

  // Bars in the plot: the running run is green (--state-running, light theme), not black.
  const bars = await page.getByTestId('gantt-plot').locator('g[data-tip="segment"] > rect:first-child').evaluateAll((els) => els.map((e) => getComputedStyle(e).fill))
  expect(bars.length).toBeGreaterThanOrEqual(2)
  for (const fill of bars) expect(black, `bar fill ${fill}`).not.toContain(fill)
  expect(bars).toContain('rgb(22, 163, 74)')
  // The lane dot is a CSS background, which also needs the variable.
  const dots = await lanes.locator('span.rounded-full').evaluateAll((els) => els.map((e) => getComputedStyle(e).backgroundColor))
  expect(dots).toContain('rgb(22, 163, 74)')
  expect(errors).toEqual([])
})

test('the legend of an empty timeline has its colours too', async ({ page }) => {
  // Regression: the --state-* variables used to be injected by the Gantt, which an empty session does not render.
  await page.goto(`/sessions/${process.env.E2E_REAL_EMPTY_SESSION}`)
  await expect(page.getByText('No runs in this session yet')).toBeVisible()
  const swatches = page.getByRole('list', { name: 'Run states' }).locator('svg > rect:first-child')
  await expect(swatches).toHaveCount(10)
  const fills = await swatches.evaluateAll((els) => els.map((e) => getComputedStyle(e).fill))
  for (const fill of fills) expect(['rgb(0, 0, 0)', 'rgba(0, 0, 0, 0)'], `fill ${fill}`).not.toContain(fill)
  expect(new Set(fills).size).toBe(10)
})

test('the run panel opens from a lane', async ({ page }) => {
  const errors = watchErrors(page)
  await page.goto(`/sessions/${session()}`)
  await page.getByRole('list', { name: 'Runs' }).getByRole('listitem').filter({ hasText: 'long-refactor' }).click()
  await expect(page).toHaveURL(new RegExp(`/runs/${runs().running}`))
  for (const tab of ['Brief', 'Route', 'Events', 'Transcript']) {
    await expect(page.getByRole('tab', { name: tab })).toBeVisible()
  }
  await page.getByRole('tab', { name: 'Events' }).click()
  await page.waitForLoadState('networkidle')
  await expect(page.getByText('This page failed to load')).toHaveCount(0)
  expect(errors).toEqual([])
})

test('inspection-only CLI calls leave no empty sessions behind', async ({ request }) => {
  const page = await (await request.get('/api/v1/sessions')).json()
  expect(page.items.map((s: { client: string }) => s.client).sort()).toEqual(['e2e-empty', 'e2e-seed'])
})


test('a CLI login code can be exchanged only once', async ({ request }) => {
  const code = new URLSearchParams(new URL(freshLoginUrl()).hash.slice(1)).get('token')
  expect((await request.post('/api/v1/token', { data: { token: code } })).status()).toBe(204)
  expect((await request.post('/api/v1/token', { data: { token: code } })).status()).toBe(401)
})
