# Playwright smoke tests

The specs run against the Vite dev server in mock mode (`vite --mode mock`), where MSW serves the API fixtures. No supervisor is needed. Playwright always starts its own fresh server (port 47911), so a leftover dev server is never reused.

```sh
pnpm install
pnpm e2e:install   # once: downloads Chromium into ~/.cache/ms-playwright
pnpm e2e           # runs e2e/*.spec.ts
pnpm e2e --ui      # optional: the Playwright UI runner
```

Projects: `smoke` (desktop) and `responsive-375`, `-768`, `-1280`. `smoke.spec.ts` also covers the Levels page: the seeded levels list their model refs and "Add model" appends a ref with a live preview. The responsive project visits every route, saves a full-page screenshot under `test-results/`, and asserts no horizontal page scroll, an off-canvas sidebar behind a trigger below 768 px, and 40 px touch targets on header controls, page actions and form buttons at 375 px. Wide tables scroll inside their own frame (`ScrollTable`) with the first column sticky.

## Real server (`real` project)

`pnpm e2e:real` (`E2E_REAL=1`) swaps the mocks for the real thing, so it catches response shapes the fixtures get wrong and styling that breaks only under the server's CSP. `e2e/real/global-setup.ts` builds `web/dist`, builds the debug binary plus the test-support `e2e-seed` and `fake-harness` (needs a Rust toolchain), starts a supervisor on a temporary config dir, socket and `HOME`, seeds runs through scripted fake harnesses (no real harness ever runs) and starts `superorchestrator ui --no-open` on a free port. Specs: every route renders without console errors or an error boundary, `/routing` lists the seeded levels (`trivial` to `expert`) with their model refs, the timeline draws non-black state colours (also for an empty session), the run panel opens, and CLI inspection calls leave no empty session. CI runs it as `browser-e2e-real`.

On CI (`CI=1`) it uses one worker and the GitHub and HTML reporters.

Failed tests keep a trace under `test-results/` (`pnpm exec playwright show-trace <trace.zip>`).

These are not part of `pnpm test` (Vitest excludes `e2e/`) and run in CI as `browser e2e` (mock) and `browser e2e (real server)`. The 10 000-event timeline pan check from design section 8 is a later follow-up.
