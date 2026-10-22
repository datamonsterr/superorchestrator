# Playwright smoke tests

The specs run against the Vite dev server in mock mode (`vite --mode mock`), where MSW serves the API fixtures. No supervisor is needed. Playwright starts the server itself (port 5199) and reuses one that is already running outside CI.

```sh
pnpm install
pnpm e2e:install   # once: downloads Chromium into ~/.cache/ms-playwright
pnpm e2e           # runs e2e/*.spec.ts
pnpm e2e --ui      # optional: the Playwright UI runner
```

Failed tests keep a trace under `test-results/` (`pnpm exec playwright show-trace <trace.zip>`).

These are not part of `pnpm test` (Vitest excludes `e2e/`) and are not wired into CI yet. The 10 000-event timeline pan check from design section 8 needs the real server with a seeded `state.db` and is a later follow-up.
