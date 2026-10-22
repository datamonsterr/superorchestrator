import { defineConfig, devices } from '@playwright/test'

// A rarely used port, and always a fresh server, so a leftover dev server can never be mistaken for the mock.
const PORT = 47911
const ci = !!process.env.CI

/** Specs run against the Vite dev server in mock mode (MSW serves the API). */
export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  fullyParallel: true,
  forbidOnly: ci,
  retries: ci ? 1 : 0,
  workers: ci ? 1 : undefined,
  reporter: ci ? [['github'], ['html', { open: 'never' }]] : [['list']],
  use: { baseURL: `http://127.0.0.1:${PORT}`, trace: 'retain-on-failure' },
  projects: [
    { name: 'smoke', testMatch: 'smoke.spec.ts', use: { ...devices['Desktop Chrome'] } },
    // Responsive checks at the widths of design 7.3 (sm, md, lg).
    ...([375, 768, 1280] as const).map((width) => ({
      name: `responsive-${width}`,
      testMatch: 'responsive.spec.ts',
      use: { ...devices['Desktop Chrome'], viewport: { width, height: width === 375 ? 740 : 900 }, hasTouch: width === 375 },
    })),
  ],
  webServer: {
    command: `pnpm exec vite --mode mock --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
})
