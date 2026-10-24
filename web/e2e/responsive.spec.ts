import { expect, test, type Page } from '@playwright/test'

const ROUTES: Array<{ path: string; heading: RegExp | string; ready: (p: Page) => ReturnType<Page['waitForSelector']> | Promise<unknown> }> = [
  { path: '/dashboard', heading: 'Dashboard', ready: (p) => p.getByRole('row', { name: /codex-acc1/ }).first().waitFor() },
  { path: '/sessions', heading: 'Sessions', ready: (p) => p.getByRole('row', { name: /claude-code/ }).first().waitFor() },
  { path: '/sessions/sess-1', heading: 'Session timeline', ready: (p) => p.getByRole('list', { name: 'Runs' }).waitFor() },
  { path: '/accounts', heading: 'Accounts', ready: (p) => p.getByRole('row', { name: /agy-acc1/ }).waitFor() },
  { path: '/routing', heading: 'Levels and routing', ready: (p) => p.getByRole('region', { name: 'Level complex' }).waitFor() },
  { path: '/agents', heading: 'Agents', ready: (p) => p.getByRole('row', { name: /reviewer/ }).waitFor() },
  { path: '/agents/reviewer', heading: 'reviewer', ready: (p) => p.getByRole('textbox', { name: 'AGENT.md' }).waitFor() },
  { path: '/agents/new', heading: 'New agent', ready: (p) => p.getByRole('textbox', { name: 'AGENT.md' }).waitFor() },
  { path: '/workflows', heading: 'Workflows', ready: (p) => p.getByRole('row', { name: /ship-feature/ }).waitFor() },
  { path: '/workflows/wf-1', heading: /run$/, ready: (p) => p.getByRole('row', { name: /implement/ }).waitFor() },
  { path: '/settings', heading: 'Settings', ready: (p) => p.getByLabel('Global run cap').waitFor() },
]

const slug = (path: string) => path.replace(/^\//, '').replace(/\//g, '-')

for (const route of ROUTES) {
  test(`${route.path} fits the viewport`, async ({ page }, testInfo) => {
    const width = page.viewportSize()!.width
    await page.goto(route.path)
    await expect(page.getByRole('heading', { level: 1, name: route.heading })).toBeVisible()
    await route.ready(page)
    await page.screenshot({ path: testInfo.outputPath(`${slug(route.path)}-${width}.png`), fullPage: true })

    // No horizontal page scroll: wide tables scroll inside their own frame.
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
    expect(overflow, 'horizontal page overflow in px').toBeLessThanOrEqual(0)

    if (width < 768) {
      // The sidebar is off-canvas behind a trigger.
      await expect(page.getByRole('complementary', { name: 'Sidebar' })).toBeHidden()
      const trigger = page.getByRole('button', { name: 'Toggle navigation' })
      await expect(trigger).toBeVisible()
      // Touch targets of at least 40 px on the header controls and the page actions.
      const targets = page.locator('[data-slot=app-header] button, [data-slot=page-actions] button, [data-slot=page-actions] a, [data-slot=form-actions] button')
      const boxes = await targets.evaluateAll((els) => els.map((e) => ({ name: e.textContent || e.getAttribute('aria-label') || e.tagName, ...(e as HTMLElement).getBoundingClientRect().toJSON() })))
      expect(boxes.length, 'primary controls measured').toBeGreaterThanOrEqual(3)
      for (const b of boxes.filter((x) => x.width > 0)) {
        expect(b.height, `${b.name} height`).toBeGreaterThanOrEqual(40)
        expect(b.width, `${b.name} width`).toBeGreaterThanOrEqual(40)
      }
    }
  })
}

test('the off-canvas sidebar opens from its trigger at 375 px', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) >= 768, 'only narrow screens use the sheet')
  await page.goto('/dashboard')
  await page.getByRole('button', { name: 'Toggle navigation' }).click()
  const nav = page.getByRole('dialog').getByRole('navigation', { name: 'Main' })
  await expect(nav).toBeVisible()
  await nav.getByRole('link', { name: 'Agents' }).click()
  await expect(page.getByRole('heading', { level: 1, name: 'Agents' })).toBeVisible()
})
