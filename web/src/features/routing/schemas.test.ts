import { describe, expect, it } from 'vitest'
import { choiceSchema, fromChoice, parseRoles, toChoice } from './schemas'

const base = { id: 'codex-low', harness: 'codex', model: 'gpt-5', effort: '', roles: 'worker', description: '', enabled: true } as const

describe('choice schema', () => {
  const schema = choiceSchema(['taken'])
  it('accepts a valid choice and rejects bad ids, duplicates, empty model and roles', () => {
    expect(schema.safeParse(base).success).toBe(true)
    expect(schema.safeParse({ ...base, id: 'Bad Id' }).success).toBe(false)
    expect(schema.safeParse({ ...base, id: 'taken' }).success).toBe(false)
    expect(schema.safeParse({ ...base, model: '  ' }).success).toBe(false)
    expect(schema.safeParse({ ...base, roles: ' , ' }).success).toBe(false)
  })
})

describe('roles and mapping', () => {
  it('parses csv roles', () => expect(parseRoles('code, review,,code')).toEqual(['code', 'review']))
  it('round-trips a choice and drops an empty effort', () => {
    const c = { id: 'a', harness: 'agy' as const, model: 'm', roles: ['x', 'y'], description: 'd', enabled: false }
    expect(toChoice(fromChoice(c))).toEqual(c)
    expect(toChoice({ ...fromChoice(c), effort: 'high' }).effort).toBe('high')
  })
})
