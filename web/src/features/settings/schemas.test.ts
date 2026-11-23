import { describe, expect, it } from 'vitest'
import * as fx from '@/api/msw/fixtures'
import { deciderPatch, deciderSchema, diffToPatch, durationSeconds, endpointError, FIELD_PATHS, settingsSchema, valuesFromConfig, valuesFromDecider } from './schemas'

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

  it('rejects out-of-range values', () => {
    const bad = structuredClone(base)
    bad.limits.global_cap = 0
    bad.ui.port = 70000
    const r = settingsSchema.safeParse(bad)
    expect(r.success).toBe(false)
  })

  it('leaves router.levels out of the form and out of every patch', () => {
    expect(fx.config.config.router?.levels).toHaveLength(2)
    expect(JSON.stringify(base)).not.toContain('levels')
    const next = structuredClone(base)
    expect(diffToPatch(base, next)).toEqual({})
  })

  it('sends several comma-separated reserved accounts as a trimmed list', () => {
    const next = structuredClone(base)
    next.orchestrator.reserved_accounts = ' codex-acc1 , agy-acc1,, '
    expect(diffToPatch(base, next)).toEqual({ orchestrator: { reserved_accounts: ['codex-acc1', 'agy-acc1'] } })
  })

  it('sends an empty list when the reserved accounts are cleared', () => {
    const before = structuredClone(base)
    before.orchestrator.reserved_accounts = 'codex-acc1'
    const next = structuredClone(before)
    next.orchestrator.reserved_accounts = ''
    expect(diffToPatch(before, next)).toEqual({ orchestrator: { reserved_accounts: [] } })
  })

  it('sends no patch for a whitespace-only reserved accounts edit', () => {
    const next = structuredClone(base)
    next.orchestrator.reserved_accounts = '   '
    expect(diffToPatch(base, next)).toEqual({})
  })

  it('lists every field path from the schema', () => {
    expect(FIELD_PATHS).not.toContain('router.jev.key_file')
    expect(FIELD_PATHS).toContain('limits.global_cap')
    expect(FIELD_PATHS).toContain('orchestrator.reserved_accounts')
    expect(FIELD_PATHS).toHaveLength(12)
  })
})

describe('decider schema', () => {
  const base = valuesFromDecider(fx.decider)
  it('accepts the fixture and mirrors the server rules', () => {
    expect(deciderSchema.safeParse(base).success).toBe(true)
    expect(endpointError('https://x.test/v1')).toBeNull()
    expect(endpointError('http://127.0.0.1:1/v1')).toBeNull()
    expect(endpointError('http://[::1]:1/v1')).toBeNull()
    expect(endpointError('http://evil.test')).not.toBeNull()
    expect(endpointError('ftp://localhost')).not.toBeNull()
    expect(endpointError('nope')).not.toBeNull()
    expect(durationSeconds('1m30s')).toBe(90)
    expect(durationSeconds('500ms')).toBe(0.5)
    expect(durationSeconds('20')).toBeNull()
  })
  it('never prefills the pasted key', () => {
    expect(base.key_paste).toBe('')
    expect(base.key_mode).toBe('file')
  })
  it('requires the chosen key source value', () => {
    expect(deciderSchema.safeParse({ ...base, key_mode: 'env', key_env: '' }).success).toBe(false)
    expect(deciderSchema.safeParse({ ...base, key_mode: 'file', key_file: '' }).success).toBe(false)
  })
  it('diffs to only the changed fields and the active key source', () => {
    expect(deciderPatch(base, base)).toEqual({})
    expect(deciderPatch(base, { ...base, timeout: '30s', key_mode: 'paste', key_paste: 'k' })).toEqual({ timeout: '30s', key: { paste: 'k' } })
    expect(deciderPatch(base, { ...base, key_mode: 'paste', key_paste: '' })).toEqual({})
    expect(deciderPatch(base, { ...base, key_env: 'ignored-in-file-mode' })).toEqual({})
  })
})
