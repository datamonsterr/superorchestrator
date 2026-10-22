import { describe, expect, it } from 'vitest'
import * as fx from '@/api/msw/fixtures'
import { diffToPatch, FIELD_PATHS, KEY_FILE_CLEARED, settingsSchema, valuesFromConfig } from './schemas'

describe('settings schemas', () => {
  const base = valuesFromConfig(fx.config)

  it('builds valid form values from the config view', () => {
    expect(settingsSchema().safeParse(base).success).toBe(true)
    expect(base.limits.global_cap).toBe(8)
  })

  it('sends only the changed leaves, nested', () => {
    const next = structuredClone(base)
    next.limits.global_cap = 4
    next.ui.port = 9000
    expect(diffToPatch(base, next)).toEqual({ limits: { global_cap: 4 }, ui: { port: 9000 } })
    expect(diffToPatch(base, base)).toEqual({})
  })

  it('does not send a cleared key_file', () => {
    const withFile = structuredClone(base)
    withFile.router.jev.key_file = '/etc/or.env'
    const cleared = structuredClone(withFile)
    cleared.router.jev.key_file = ''
    expect(diffToPatch(withFile, cleared)).toEqual({})
  })

  it('rejects out-of-range values', () => {
    const bad = structuredClone(base)
    bad.router.policy.min_confidence = 1.5
    bad.ui.port = 70000
    const r = settingsSchema().safeParse(bad)
    expect(r.success).toBe(false)
  })

  it('rejects clearing a saved key_file, and allows an empty one that was never set', () => {
    const withFile = structuredClone(base)
    withFile.router.jev.key_file = '/etc/or.env'
    const cleared = structuredClone(withFile)
    cleared.router.jev.key_file = ''
    const r = settingsSchema('/etc/or.env').safeParse(cleared)
    expect(r.success).toBe(false)
    expect(!r.success && r.error.issues[0]).toMatchObject({ path: ['router', 'jev', 'key_file'], message: KEY_FILE_CLEARED })
    expect(settingsSchema('').safeParse(cleared).success).toBe(true)
  })

  it('matches the key_file rule by path, not by leaf name', () => {
    const a = structuredClone(base)
    const b = structuredClone(base)
    b.router.jev.key_file = '/x'
    expect(diffToPatch(a, b)).toEqual({ router: { jev: { key_file: '/x' } } })
  })

  it('lists every field path from the schema', () => {
    expect(FIELD_PATHS).toContain('router.jev.key_file')
    expect(FIELD_PATHS).toContain('limits.global_cap')
    expect(FIELD_PATHS).toHaveLength(15)
  })
})
