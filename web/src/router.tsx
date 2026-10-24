import type { ComponentType } from 'react'
import { createBrowserRouter, createMemoryRouter, Navigate, type RouteObject } from 'react-router'
import { AppShell } from '@/components/layout/AppShell'
import { NotFoundPage } from '@/components/layout/NotFoundPage'
import { RouteErrorBoundary } from '@/components/layout/RouteErrorBoundary'
import { PageFallback } from '@/components/layout/PageFallback'
import { TokenPage } from '@/features/auth/TokenPage'

type Loader = () => Promise<Record<string, unknown>>

/** Lazy page route: code splits the page, with a per-route error boundary. */
function page(path: string | undefined, load: Loader, name: string): RouteObject {
  return {
    path,
    index: path === undefined,
    lazy: async () => ({ Component: (await load())[name] as ComponentType }),
    ErrorBoundary: RouteErrorBoundary,
  } as RouteObject
}

const timeline = () => import('@/features/timeline/TimelinePage')

export const routes: RouteObject[] = [
  { path: '/token', element: <TokenPage />, ErrorBoundary: RouteErrorBoundary },
  {
    path: '/',
    element: <AppShell />,
    ErrorBoundary: RouteErrorBoundary,
    HydrateFallback: PageFallback,
    children: [
      { index: true, element: <Navigate to="/dashboard" replace /> },
      page('dashboard', () => import('@/features/dashboard/DashboardPage'), 'DashboardPage'),
      page('sessions', () => import('@/features/sessions/SessionsPage'), 'SessionsPage'),
      page('sessions/:sessionId', timeline, 'TimelinePage'),
      page('sessions/:sessionId/runs/:runId', timeline, 'TimelinePage'),
      page('accounts', () => import('@/features/accounts/AccountsPage'), 'AccountsPage'),
      page('routing', () => import('@/features/routing/RoutingPage'), 'RoutingPage'),
      page('agents', () => import('@/features/agents/AgentsPage'), 'AgentsPage'),
      page('agents/:name', () => import('@/features/agents/AgentEditor'), 'AgentEditor'),
      page('workflows', () => import('@/features/workflows/WorkflowsPage'), 'WorkflowsPage'),
      page('workflows/:id', () => import('@/features/workflows/WorkflowRunPage'), 'WorkflowRunPage'),
      page('settings', () => import('@/features/settings/SettingsPage'), 'SettingsPage'),
      { path: '*', element: <NotFoundPage />, ErrorBoundary: RouteErrorBoundary },
    ],
  },
]

export const createAppRouter = () => createBrowserRouter(routes)
export const createTestRouter = (initialEntries: string[] = ['/']) => createMemoryRouter(routes, { initialEntries })
