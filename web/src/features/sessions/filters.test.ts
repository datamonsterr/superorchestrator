import { describe, expect, it } from 'vitest'
import { filterFromParams, paramsFromFilter } from './filters'

describe('session filters', () => {
  it('parses status and q', () => {
    expect(filterFromParams(new URLSearchParams('status=failed&q=repo'))).toEqual({ status: 'failed', q: 'repo' })
  })
  it('ignores unknown status and blank q', () => {
    expect(filterFromParams(new URLSearchParams('status=done&q=%20'))).toEqual({})
  })
  it('round-trips and omits empties', () => {
    expect(paramsFromFilter({ status: 'active' }).toString()).toBe('status=active')
    expect(paramsFromFilter({}).toString()).toBe('')
  })
})
