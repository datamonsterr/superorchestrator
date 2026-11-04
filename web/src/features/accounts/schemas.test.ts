import { describe, expect, it } from 'vitest'
import { addAccountSchema, buildAddAccountRequest, editAccountSchema, flagSchema } from './schemas'

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

describe('token trimming', () => {
  it('trims a pasted key in the schema and the builder', () => {
    const r = addAccountSchema([]).safeParse({ harness: 'claude', name: 'z', endpointPreset: 'zai', tokenSource: 'paste', token: '  sk-1 \n' })
    expect(r.success && r.data.token).toBe('sk-1')
    expect(buildAddAccountRequest({ harness: 'claude', name: 'z', endpointPreset: 'zai', tokenSource: 'paste', token: ' sk-2 ' }).token).toBe('sk-2')
  })
  it('rejects a whitespace-only key', () => {
    expect(addAccountSchema([]).safeParse({ harness: 'claude', name: 'z', endpointPreset: 'zai', tokenSource: 'paste', token: '   ' }).success).toBe(false)
  })
})

describe('buildAddAccountRequest', () => {
  const base = { harness: 'claude', name: 'z' } as const
  it('sends only harness and name without an endpoint', () => {
    expect(buildAddAccountRequest({ ...base, endpointPreset: 'none', token: 'leak', baseUrl: 'x' })).toEqual({ harness: 'claude', name: 'z' })
    expect(buildAddAccountRequest({ harness: 'codex', name: 'c', home: '/h', endpointPreset: 'zai', token: 'leak' })).toEqual({ harness: 'codex', name: 'c', home: '/h' })
  })
  it('sends a preset with a pasted token', () => {
    expect(buildAddAccountRequest({ ...base, endpointPreset: 'kimi', tokenSource: 'paste', token: 'k', baseUrl: 'ignored' })).toEqual({
      harness: 'claude',
      name: 'z',
      endpoint: { preset: 'kimi' },
      token: 'k',
    })
  })
  it('sends token_env and no token for the env variant', () => {
    const r = buildAddAccountRequest({ ...base, endpointPreset: 'zai', tokenSource: 'env', tokenEnv: 'ZAI_API_KEY', token: 'stale' })
    expect(r).toEqual({ harness: 'claude', name: 'z', endpoint: { preset: 'zai', token_env: 'ZAI_API_KEY' } })
    expect('token' in r).toBe(false)
  })
  it('sends base url, models and slots for custom', () => {
    expect(
      buildAddAccountRequest({ ...base, endpointPreset: 'custom', tokenSource: 'paste', token: 'k', baseUrl: 'https://x.test', models: 'a, b,,', slotOpus: 'a', slotSonnet: 'a', slotHaiku: 'b' }),
    ).toEqual({
      harness: 'claude',
      name: 'z',
      endpoint: { preset: 'custom', base_url: 'https://x.test', models: ['a', 'b'], slots: { opus: 'a', sonnet: 'a', haiku: 'b' } },
      token: 'k',
    })
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
