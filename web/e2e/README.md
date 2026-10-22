# Playwright smoke tests

The specs run against the Vite dev server in mock mode (`vite --mode mock`), where MSW serves the API fixtures. No supervisor is needed. Playwright always starts its own fresh server (port 47911), so a leftover dev server is never reused.

```sh
pnpm install
pnpm e2e:install   # once: downloads Chromium into ~/.cache/ms-playwright
pnpm e2e           # runs e2e/*.spec.ts
pnpm e2e --ui      # optional: the Playwright UI runner
```

Projects: `smoke` (desktop) and `responsive-375`, `-768`, `-1280`. The responsive project visits every route, saves a full-page screenshot under `test-results/`, and asserts no horizontal page scroll, an off-canvas sidebar behind a trigger below 768 px, and 40 px touch targets on header controls, page actions and form buttons at 375 px. Wide tables scroll inside their own frame (`ScrollTable`) with the first column sticky.

On CI (`CI=1`) it uses one worker and the GitHub and HTML reporters.

Failed tests keep a trace under `test-results/` (`pnpm exec playwright show-trace <trace.zip>`).

These are not part of `pnpm test` (Vitest excludes `e2e/`) and are not wired into CI yet. The 10 000-event timeline pan check from design section 8 needs the real server with a seeded `state.db` and is a later follow-up.
