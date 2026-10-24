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

test('routing page lists levels and adds a model', async ({ page }) => {
  await page.goto('/routing')
  await expect(page.getByRole('heading', { level: 1, name: 'Levels and routing' })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Level complex' }).getByText('codex/gpt-6.1-sol/high')).toBeVisible()

  const simple = page.getByRole('region', { name: 'Level simple' })
  await simple.getByRole('button', { name: 'Add model' }).click()
  const dialog = page.getByRole('dialog', { name: 'Add model to simple' })
  await dialog.getByRole('combobox', { name: 'Harness' }).click()
  await page.getByRole('option', { name: 'codex' }).click()
  await dialog.getByLabel('Model', { exact: true }).fill('gpt-6.1-sol')
  await dialog.getByLabel('Effort (optional)').fill('low')
  await expect(dialog.getByText('codex/gpt-6.1-sol/low', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Save model' }).click()
  await expect(dialog).toBeHidden()
  await expect(simple.getByText('codex/gpt-6.1-sol/low', { exact: true })).toBeVisible()
})
