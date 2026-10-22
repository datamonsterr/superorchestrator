import { describe, expect, it } from 'vitest'
import * as fx from '@/api/msw/fixtures'
import { diffToPatch, settingsSchema, valuesFromConfig } from './schemas'

describe('settings schemas', () => {
  const base = valuesFromConfig(fx.config)

  it('builds valid form values from the config view', () => {
    expect(settingsSchema.safeParse(base).success).toBe(true)
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
    const r = settingsSchema.safeParse(bad)
    expect(r.success).toBe(false)
  })
})
