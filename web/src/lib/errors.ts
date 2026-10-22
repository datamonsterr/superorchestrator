import { ApiError } from '@/api/client'

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Something went wrong'
}

/** True when the endpoint does not exist on this supervisor (404 or 501). */
export const isNotAvailable = (error: unknown): boolean => error instanceof ApiError && error.isNotAvailable

/**
 * The route itself is missing: 501, or a 404 from the web layer (string code, so `ApiError.code` is 0).
 * A 404 with a supervisor code means a named thing (an agent, a run) does not exist.
 */
export const isEndpointMissing = (error: unknown): boolean =>
  error instanceof ApiError && (error.status === 501 || (error.status === 404 && error.code === 0))
