import type { FieldValues, Path, UseFormSetError } from 'react-hook-form'
import { ApiError } from '@/api/client'

/**
 * Maps a 422 `data` object ({ field: message | message[] }) onto form fields.
 * Returns true when at least one field took an error; otherwise the caller shows
 * the message in a form-level alert (the global mutation toast also fires).
 */
export function applyApiFieldErrors<T extends FieldValues>(error: unknown, setError: UseFormSetError<T>, fields: readonly string[]): boolean {
  if (!(error instanceof ApiError) || error.status !== 422) return false
  const data = error.data
  if (!data || typeof data !== 'object' || Array.isArray(data)) return false
  let applied = false
  for (const [key, value] of Object.entries(data as Record<string, unknown>)) {
    const message = Array.isArray(value) ? value.filter((v) => typeof v === 'string').join(', ') : value
    if (typeof message === 'string' && message && fields.includes(key)) {
      setError(key as Path<T>, { type: 'server', message })
      applied = true
    }
  }
  return applied
}
