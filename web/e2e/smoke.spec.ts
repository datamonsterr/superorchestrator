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
  await page.getByRole('option', { name: 'claude', exact: true }).click()
  await dialog.getByLabel('Model', { exact: true }).fill('claude-haiku-5-5')
  await dialog.getByLabel('Effort (optional)').fill('low')
  await expect(dialog.getByText('claude/claude-haiku-5-5/low', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Save model' }).click()
  await expect(dialog).toBeHidden()
  await expect(simple.getByText('claude/claude-haiku-5-5/low', { exact: true })).toBeVisible()
})

test('settings decider tab: change the model, save, then test the decider', async ({ page }) => {
  await page.goto('/settings')
  await page.getByRole('tab', { name: 'Decider' }).click()
  const model = page.getByLabel('Model', { exact: true })
  await expect(model).toHaveValue('openai/gpt-5-mini')
  await model.fill('anthropic/claude-haiku-5-5')
  await page.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Decider settings saved')).toBeVisible()
  await expect(model).toHaveValue('anthropic/claude-haiku-5-5')
  await expect(page.getByTestId('decider-effective')).toContainText('anthropic/claude-haiku-5-5')

  await page.getByRole('button', { name: 'Test decider' }).click()
  const result = page.getByRole('status', { name: 'Test result' })
  await expect(result).toContainText('62.5')
  await expect(result).toContainText('high')
  await expect(result).toContainText('412 ms')
})
