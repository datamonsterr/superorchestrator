import { expect, test } from '@playwright/test'

test('dashboard loads', async ({ page }) => {
  await page.goto('/dashboard')
  await expect(page.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible()
})

test('open a session timeline, click a lane and see the panel tabs', async ({ page }) => {
  await page.goto('/sessions')
  await page.getByRole('link', { name: /Open session claude-code sess-1/ }).click()
  const lanes = page.getByRole('list', { name: 'Runs' }).getByRole('listitem')
  await expect(lanes.first()).toBeVisible()
  await lanes.nth(1).click()
  await expect(page).toHaveURL(/\/runs\//)
  for (const tab of ['Brief', 'Route', 'Events', 'Transcript']) {
    await expect(page.getByRole('tab', { name: tab })).toBeVisible()
  }
})

test('add account dialog leads to the login command', async ({ page }) => {
  await page.goto('/accounts')
  await page.getByRole('button', { name: 'Add account' }).first().click()
  const dialog = page.getByRole('dialog')
  await dialog.getByRole('combobox', { name: 'Harness' }).click()
  await page.getByRole('option', { name: 'Claude' }).click()
  await dialog.getByLabel('Name').fill('claude-acc9')
  await dialog.getByRole('button', { name: 'Add and sign in' }).click()
  await expect(page.getByText('claude login')).toBeVisible()
})

test('dark mode toggle', async ({ page }) => {
  await page.goto('/dashboard')
  await page.getByRole('button', { name: /^Theme/ }).click()
  await page.getByRole('menuitemradio', { name: 'Dark' }).click()
  await expect(page.locator('html')).toHaveClass(/dark/)
  await page.getByRole('button', { name: /^Theme/ }).click()
  await page.getByRole('menuitemradio', { name: 'Light' }).click()
  await expect(page.locator('html')).not.toHaveClass(/dark/)
})

test('routing page shows the Levels tab with its models', async ({ page }) => {
  await page.goto('/choices?tab=levels')
  await expect(page.getByRole('tab', { name: 'Levels', selected: true })).toBeVisible()
  await expect(page.getByText('Using the default levels')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Add level' })).toBeVisible()
})
