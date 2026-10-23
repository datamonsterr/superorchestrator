import { render } from '@testing-library/react'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { afterEach, expect, it } from 'vitest'
import { ChartStyle } from './chart'
import { ScrollArea } from './scroll-area'

const require = createRequire(import.meta.url)
afterEach(() => document.querySelector('meta[name="so-style-nonce"]')?.remove())

it('attaches the document nonce to chart and scroll-area style elements', () => {
  const meta = document.createElement('meta')
  meta.name = 'so-style-nonce'
  meta.content = 'per-document-random-nonce'
  document.head.append(meta)
  const { container } = render(<>
    <ChartStyle id="test" config={{ cost: { color: 'red' } }} />
    <ScrollArea>Scrollable content</ScrollArea>
  </>)
  const styles = container.querySelectorAll('style')
  expect(styles).toHaveLength(2)
  for (const style of styles) expect(style).toHaveAttribute('nonce', meta.content)
})

it('allows only the exact fixed stylesheet injected by the installed Sonner version', () => {
  const module = readFileSync(require.resolve('sonner').replace(/index\.js$/, 'index.mjs'), 'utf8')
  const literal = module.match(/__insertCSS\(("(?:\\.|[^"\\])*")\)/)?.[1]
  expect(literal).toBeDefined()
  const hash = createHash('sha256').update(JSON.parse(literal!)).digest('base64')
  const policy = readFileSync('../crates/so-web/src/security.rs', 'utf8')
  expect(policy).toContain(`'sha256-${hash}'`)
  // Sonner appends an empty style element before filling it with the fixed CSS.
  expect(policy).toContain(`'sha256-${createHash('sha256').update('').digest('base64')}'`)
})
