/// <reference types="node" />
import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { stateCss } from './colors'

describe('state-colors.css', () => {
  it('state-colors.css (imported by index.css) is the generated CSS', () => {
    const file = resolve(process.cwd(), 'src/features/timeline/state-colors.css')
    if (process.env.UPDATE_STATE_CSS) writeFileSync(file, stateCss())
    expect(readFileSync(file, 'utf8')).toBe(stateCss())
  })

  it('index.css imports state-colors.css, so the variables exist on every page', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/index.css'), 'utf8')
    expect(css).toContain('@import "./features/timeline/state-colors.css";')
  })
})
