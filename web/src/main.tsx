import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

async function boot() {
  // `pnpm dev:mock` serves the API from MSW (?large=1 adds the 10k-event session).
  if (import.meta.env.MODE === 'mock') {
    const { startMockWorker } = await import('@/api/msw/browser')
    await startMockWorker()
  }
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}
void boot()
