import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import { applyTheme, readStoredTheme } from '@/lib/theme'
import App from './App.tsx'

// Apply the stored theme before first paint to avoid a flash.
applyTheme(readStoredTheme())

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
