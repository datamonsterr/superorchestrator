import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { configDefaults, defineConfig, type Plugin } from 'vitest/config'

const require = createRequire(import.meta.url)

/** Serves msw's worker script in `--mode mock` only, so it never lands in dist. */
function mockServiceWorker(): Plugin {
  return {
    name: 'so:mock-service-worker',
    apply: (_config, env) => env.mode === 'mock' && env.command === 'serve',
    configureServer(server) {
      server.middlewares.use('/mockServiceWorker.js', (_req, res) => {
        res.setHeader('Content-Type', 'text/javascript')
        res.end(fs.readFileSync(require.resolve('msw/mockServiceWorker.js')))
      })
    },
  }
}

const vendor = (name: string, packages: string[]) => ({
  name,
  test: new RegExp(`node_modules[\\\\/](\\.pnpm[\\\\/][^\\\\/]*[\\\\/]node_modules[\\\\/])?(${packages.join('|')})[\\\\/]`),
  priority: 10,
})

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss(), mockServiceWorker()],
  resolve: {
    alias: { '@': path.resolve(import.meta.dirname, './src') },
  },
  server: {
    proxy: {
      // Includes SSE: http-proxy streams responses without buffering.
      '/api': { target: 'http://127.0.0.1:7878', changeOrigin: true, timeout: 0, proxyTimeout: 0 },
    },
  },
  build: {
    outDir: 'dist',
    rolldownOptions: {
      output: {
        // Stable vendor chunks. No catch-all group: lazy-only libraries (recharts) must stay with their pages.
        codeSplitting: {
          groups: [
            vendor('react', ['react', 'react-dom', 'scheduler']),
            vendor('radix', ['radix-ui', '@radix-ui', '@floating-ui', 'aria-hidden', 'react-remove-scroll']),
            vendor('router', ['react-router']),
            vendor('query', ['@tanstack/react-query', '@tanstack/query-core']),
          ],
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    globals: false,
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    // Playwright specs run with `pnpm e2e`.
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
})
