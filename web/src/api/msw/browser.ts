import { setupWorker } from 'msw/browser'
import { createHandlers } from './handlers'

/** Starts the service worker for `pnpm dev:mock`. `?large=1` lists the 10k-event session. */
export async function startMockWorker() {
  const large = new URLSearchParams(window.location.search).get('large') === '1'
  const worker = setupWorker(...createHandlers({ large }))
  await worker.start({ onUnhandledFrame: 'bypass', quiet: true })
}
