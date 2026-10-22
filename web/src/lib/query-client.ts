import { MutationCache, QueryCache, QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { ApiError } from '@/api/client'
import { errorMessage } from '@/lib/errors'

const TOAST_EVERY_MS = 60_000

/**
 * App-wide QueryClient. Background refetch errors (a query that already has
 * data) toast at most once a minute; first-load errors render in the page.
 */
export function createQueryClient(): QueryClient {
  let lastToast = 0
  return new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        if (query.state.data === undefined) return
        if (error instanceof ApiError && error.isUnauthorized) return
        const now = Date.now()
        if (now - lastToast < TOAST_EVERY_MS) return
        lastToast = now
        toast.error('Could not refresh data', { description: errorMessage(error) })
      },
    }),
    mutationCache: new MutationCache(),
    defaultOptions: {
      queries: {
        staleTime: 5_000,
        refetchOnWindowFocus: true,
        retry: (count, error) => !(error instanceof ApiError && error.status >= 400 && error.status < 500) && count < 1,
      },
    },
  })
}
