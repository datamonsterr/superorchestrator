import { useEffect, useMemo } from 'react'
import { QueryClientProvider, type QueryClient } from '@tanstack/react-query'
import type { DataRouter } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import { ThemeProvider } from '@/components/layout/ThemeProvider'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { setUnauthorizedHandler } from '@/api/client'
import { createQueryClient } from '@/lib/query-client'
import { createAppRouter } from './router'

interface Props {
  router?: DataRouter
  queryClient?: QueryClient
}

/** Providers and the route table. Props exist so tests can inject a memory router. */
export default function App({ router: injected, queryClient }: Props) {
  const client = useMemo(() => queryClient ?? createQueryClient(), [queryClient])
  const router = useMemo(() => injected ?? createAppRouter(), [injected])

  // 401 anywhere sends the user to the token prompt.
  useEffect(
    () =>
      setUnauthorizedHandler(() => {
        if (router.state.location.pathname !== '/token') void router.navigate('/token')
      }),
    [router],
  )

  return (
    <ThemeProvider>
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <RouterProvider router={router} />
          <Toaster />
        </TooltipProvider>
      </QueryClientProvider>
    </ThemeProvider>
  )
}
