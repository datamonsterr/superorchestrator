import { describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import { applyApiFieldErrors } from './form-errors'

describe('applyApiFieldErrors', () => {
  it('maps known fields from a 422 and ignores the rest', () => {
    const setError = vi.fn()
    const err = new ApiError(422, -32602, 'bad', { model: 'unknown model', roles: ['a', 'b'], other: 'x' })
    expect(applyApiFieldErrors(err, setError, ['model', 'roles'])).toBe(true)
    expect(setError).toHaveBeenCalledWith('model', { type: 'server', message: 'unknown model' })
    expect(setError).toHaveBeenCalledWith('roles', { type: 'server', message: 'a, b' })
    expect(setError).toHaveBeenCalledTimes(2)
  })
  it('does nothing for other errors', () => {
    const setError = vi.fn()
    expect(applyApiFieldErrors(new ApiError(500, 0, 'x', { model: 'm' }), setError, ['model'])).toBe(false)
    expect(applyApiFieldErrors(new Error('x'), setError, ['model'])).toBe(false)
    expect(setError).not.toHaveBeenCalled()
  })
})
