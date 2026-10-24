import { describe, expect, it } from 'vitest'
import { buildRef, levelSchema, modelRefSchema, paramsFrom, splitRef } from './schemas'

describe('buildRef and splitRef', () => {
  it('builds harness/model[/effort] and leaves the effort out when blank', () => {
    expect(buildRef('codex', 'gpt-6.1-sol', 'high')).toBe('codex/gpt-6.1-sol/high')
    expect(buildRef('codex', ' gpt-6.1-sol ', '  ')).toBe('codex/gpt-6.1-sol')
    expect(buildRef('opencode', 'opencode/muse-spark-1.3')).toBe('opencode/opencode/muse-spark-1.3')
  })
  it.each([
    ['codex/gpt-6.1-sol/high', { harness: 'codex', model: 'gpt-6.1-sol', effort: 'high' }],
    ['agy/claude-sonnet-4-6/thinking', { harness: 'agy', model: 'claude-sonnet-4-6', effort: 'thinking' }],
    ['claude/sonnet', { harness: 'claude', model: 'sonnet', effort: '' }],
    ['opencode/opencode/muse-spark-1.3', { harness: 'opencode', model: 'opencode/muse-spark-1.3', effort: '' }],
    ['opencode/opencode/deepseek-v4.1-flash-free/low', { harness: 'opencode', model: 'opencode/deepseek-v4.1-flash-free', effort: 'low' }],
  ])('splits %s', (ref, parts) => {
    expect(splitRef(ref)).toEqual(parts)
    expect(buildRef(parts.harness, parts.model, parts.effort)).toBe(ref)
  })
})

describe('model ref schema', () => {
  const schema = modelRefSchema(['codex/gpt-6.1-sol/high'])
  const issue = (v: unknown) => {
    const r = schema.safeParse(v)
    return r.success ? null : r.error.issues[0].message
  }
  it('requires a model', () => expect(issue({ harness: 'codex', model: '  ', effort: '' })).toBe('Enter a model'))
  it('requires provider/model for opencode', () => {
    expect(issue({ harness: 'opencode', model: 'muse-spark-1.3', effort: '' })).toMatch(/provider\/model/)
    expect(issue({ harness: 'opencode', model: 'opencode/muse-spark-1.3', effort: '' })).toBeNull()
  })
  it('rejects a ref the level already lists, with or without the effort', () => {
    expect(issue({ harness: 'codex', model: 'gpt-6.1-sol', effort: 'high' })).toBe('This level already lists this model')
    expect(issue({ harness: 'codex', model: 'gpt-6.1-sol', effort: 'low' })).toBeNull()
    expect(issue({ harness: 'codex', model: 'gpt-6.1-sol', effort: '' })).toBeNull()
  })
})

describe('level schema', () => {
  const schema = levelSchema([{ name: 'low', min: 0, max: 49 }])
  const issue = (v: Record<string, unknown>) => {
    const r = schema.safeParse({ name: 'high', min: 50, max: 100, tier: 'none', ...v })
    return r.success ? null : r.error.issues[0].message
  }
  it('accepts a free range', () => expect(issue({})).toBeNull())
  it('rejects an empty or duplicate name', () => {
    expect(issue({ name: ' ' })).toBe('Enter a name')
    expect(issue({ name: 'low' })).toBe('A level with this name already exists')
  })
  it('rejects bounds outside 0 to 100, fractions and non numbers', () => {
    expect(issue({ max: 101 })).toBe('Use 0 to 100')
    expect(issue({ min: -1 })).toBe('Use 0 to 100')
    expect(issue({ min: 1.5 })).toBe('Use a whole number')
    expect(issue({ min: NaN })).toBe('Enter a start from 0 to 100')
  })
  it('rejects an inverted or overlapping range', () => {
    expect(issue({ min: 80, max: 70 })).toBe('The end must not be below the start')
    expect(issue({ min: 40, max: 100 })).toBe('Overlaps low (0–49)')
  })
})

describe('preview params', () => {
  it('sends only the filled fields, and no role', () => {
    expect(paramsFrom({ prompt: 'x', harness: 'any', account: 'any', model: 'codex/gpt-6.1-sol/high', effort: '' })).toEqual({ prompt: 'x', model: 'codex/gpt-6.1-sol/high' })
  })
})
