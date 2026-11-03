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

describe('addAccountSchema endpoint', () => {
  const schema = addAccountSchema([])
  const base = { harness: 'claude', name: 'zai-1' }
  const ok = (v: object) => schema.safeParse({ ...base, ...v }).success
  const paths = (v: object) => {
    const r = schema.safeParse({ ...base, ...v })
    return r.success ? [] : r.error.issues.map((i) => i.path.join('.'))
  }
  it('preset requires exactly one token source', () => {
    expect(ok({ endpointPreset: 'zai', tokenSource: 'paste', token: 'k' })).toBe(true)
    expect(paths({ endpointPreset: 'zai', tokenSource: 'paste', token: '' })).toContain('token')
    expect(ok({ endpointPreset: 'zai', tokenSource: 'env', tokenEnv: 'ZAI_API_KEY' })).toBe(true)
    expect(paths({ endpointPreset: 'zai', tokenSource: 'env', tokenEnv: '' })).toContain('tokenEnv')
  })
  it.each(['1BAD', 'has-dash', 'a b'])('rejects env name %j', (tokenEnv) => {
    expect(paths({ endpointPreset: 'kimi', tokenSource: 'env', tokenEnv })).toContain('tokenEnv')
  })
  it('custom requires base url, models and slots', () => {
    const p = paths({ endpointPreset: 'custom', tokenSource: 'paste', token: 'k' })
    expect(p).toEqual(expect.arrayContaining(['baseUrl', 'models', 'slotOpus', 'slotSonnet', 'slotHaiku']))
    const full = { endpointPreset: 'custom', tokenSource: 'paste', token: 'k', models: 'm1, m2', slotOpus: 'm1', slotSonnet: 'm1', slotHaiku: 'm2' }
    expect(ok({ ...full, baseUrl: 'https://llm.example.com/v1' })).toBe(true)
    expect(ok({ ...full, baseUrl: 'http://localhost:8080' })).toBe(true)
    expect(ok({ ...full, baseUrl: 'http://127.0.0.1:8080' })).toBe(true)
    expect(ok({ ...full, baseUrl: 'http://example.com' })).toBe(false)
    expect(ok({ ...full, baseUrl: 'ftp://example.com' })).toBe(false)
  })
  it('none and non-claude harnesses ignore endpoint fields', () => {
    expect(ok({ endpointPreset: 'none', tokenSource: 'paste', token: '', baseUrl: 'junk' })).toBe(true)
    expect(ok({ harness: 'codex', endpointPreset: 'custom' })).toBe(true)
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
