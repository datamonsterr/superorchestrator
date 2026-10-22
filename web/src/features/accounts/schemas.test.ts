import { describe, expect, it } from 'vitest'
import { addAccountSchema, editAccountSchema, flagSchema } from './schemas'

describe('addAccountSchema', () => {
  const schema = addAccountSchema(['codex-acc1'])
  it('accepts a valid name', () => expect(schema.safeParse({ harness: 'codex', name: 'codex-acc2' }).success).toBe(true))
  it.each(['Codex', 'a b', 'x_y', ''])('rejects %j', (name) => expect(schema.safeParse({ harness: 'codex', name }).success).toBe(false))
  it('rejects duplicates', () => {
    const r = schema.safeParse({ harness: 'codex', name: 'codex-acc1' })
    expect(r.success ? '' : r.error.issues[0].message).toMatch(/already exists/)
  })
})

describe('editAccountSchema', () => {
  const ok = { cap: 2, priority: 1, enabled: true, pools: [] }
  it('bounds the cap to 1..16', () => {
    expect(editAccountSchema.safeParse({ ...ok, cap: 0 }).success).toBe(false)
    expect(editAccountSchema.safeParse({ ...ok, cap: 17 }).success).toBe(false)
    expect(editAccountSchema.safeParse({ ...ok, cap: 16 }).success).toBe(true)
    expect(editAccountSchema.safeParse({ ...ok, cap: 1.5 }).success).toBe(false)
    expect(editAccountSchema.safeParse({ ...ok, cap: NaN }).success).toBe(false)
  })
})

describe('flagSchema', () => {
  it('needs a date only for custom durations', () => {
    expect(flagSchema.safeParse({ pool: 'all', duration: '5', until: '' }).success).toBe(true)
    expect(flagSchema.safeParse({ pool: 'all', duration: 'custom', until: '' }).success).toBe(false)
    expect(flagSchema.safeParse({ pool: 'all', duration: 'custom', until: '2026-01-02T10:00' }).success).toBe(true)
  })
})
