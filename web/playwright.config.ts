import { defineConfig, devices } from '@playwright/test'

// A rarely used port, and always a fresh server, so a leftover dev server can never be mistaken for the mock.
const PORT = 47911
const ci = !!process.env.CI
// `pnpm e2e:real` runs the `real` project against a real supervisor; plain `pnpm e2e` runs the mocked ones.
const real = !!process.env.E2E_REAL

/**
 * Specs run against the Vite dev server in mock mode (MSW serves the API), except the `real` project, which
 * runs `e2e/real` against a supervisor seeded by `e2e/real/global-setup.ts`.
 */
export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  fullyParallel: true,
  forbidOnly: ci,
  retries: ci ? 1 : 0,
  workers: ci ? 1 : undefined,
  reporter: ci ? [['github'], ['html', { open: 'never' }]] : [['list']],
  use: { baseURL: `http://127.0.0.1:${PORT}`, trace: 'retain-on-failure' },
  globalSetup: real ? './e2e/real/global-setup.ts' : undefined,
  projects: real
    ? [{ name: 'real', testMatch: 'real/*.spec.ts', use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 900 } } }]
    : [
    { name: 'smoke', testMatch: 'smoke.spec.ts', use: { ...devices['Desktop Chrome'] } },
    // Responsive checks at the widths of design 7.3 (sm, md, lg).
    ...([375, 768, 1280] as const).map((width) => ({
      name: `responsive-${width}`,
      testMatch: 'responsive.spec.ts',
      use: { ...devices['Desktop Chrome'], viewport: { width, height: width === 375 ? 740 : 900 }, hasTouch: width === 375 },
    })),
  ],
  webServer: real
    ? undefined
    : {
        command: `pnpm exec vite --mode mock --host 127.0.0.1 --port ${PORT} --strictPort`,
        url: `http://127.0.0.1:${PORT}`,
        reuseExistingServer: false,
        timeout: 60_000,
      },
})
