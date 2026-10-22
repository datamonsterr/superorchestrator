import { ApiError } from '@/api/client'

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Something went wrong'
}

/** True when the endpoint does not exist on this supervisor (404 or 501). */
export const isNotAvailable = (error: unknown): boolean => error instanceof ApiError && error.isNotAvailable
