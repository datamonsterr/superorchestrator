import { Outlet } from 'react-router'
import { SidebarProvider } from '@/components/ui/sidebar'
import { StreamContext, useEventStream } from '@/api/stream'
import { AppHeader } from './AppHeader'
import { AppSidebar } from './AppSidebar'

/** Sidebar + header + page outlet. Owns the single EventSource for the app. */
export function AppShell() {
  const stream = useEventStream()
  return (
    <StreamContext.Provider value={stream}>
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-background focus:px-3 focus:py-2 focus:ring-2 focus:ring-ring"
      >
        Skip to content
      </a>
      <SidebarProvider>
        <AppSidebar />
        <div className="flex min-h-svh min-w-0 flex-1 flex-col">
          <AppHeader />
          <main id="main" tabIndex={-1} className="min-w-0 flex-1 p-4 outline-none md:p-6">
            <Outlet />
          </main>
        </div>
      </SidebarProvider>
    </StreamContext.Provider>
  )
}
