// Bundle budget: fails when gzipped JS exceeds a threshold. Run after `pnpm build`.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'

const BUDGET_KB = { entry: 70, initialTotal: 200, anyChunk: 120 }
const dist = new URL('../dist', import.meta.url).pathname
const html = readFileSync(join(dist, 'index.html'), 'utf8')
const assets = readdirSync(join(dist, 'assets')).filter((f) => f.endsWith('.js'))
const gz = (f) => gzipSync(readFileSync(join(dist, 'assets', f))).length / 1024
const sizes = Object.fromEntries(assets.map((f) => [f, gz(f)]))

// Initial = what index.html loads plus the static imports it modulepreloads.
const initial = new Set([...html.matchAll(/assets\/([^"']+\.js)/g)].map((m) => m[1]))
const entry = [...initial].find((f) => f.startsWith('index-'))
const initialTotal = [...initial].reduce((n, f) => n + (sizes[f] ?? 0), 0)
const largest = Object.entries(sizes).sort((a, b) => b[1] - a[1])[0]

const rows = [
  ['entry chunk', sizes[entry] ?? 0, BUDGET_KB.entry],
  ['initial JS total', initialTotal, BUDGET_KB.initialTotal],
  [`largest chunk (${largest[0]})`, largest[1], BUDGET_KB.anyChunk],
]
let failed = false
for (const [name, kb, max] of rows) {
  const bad = kb > max
  failed ||= bad
  console.log(`${bad ? 'FAIL' : 'ok  '} ${name}: ${kb.toFixed(1)} kB gzip (budget ${max})`)
}
if (statSync(join(dist, 'index.html')).size === 0) failed = true
process.exit(failed ? 1 : 0)
