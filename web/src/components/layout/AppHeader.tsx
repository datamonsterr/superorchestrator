import { Separator } from '@/components/ui/separator'
import { SidebarTrigger } from '@/components/ui/sidebar'
import { CapMeter } from './CapMeter'
import { CommandPalette } from './CommandPalette'
import { SupervisorPill } from './SupervisorPill'
import { ThemeToggle } from './ThemeToggle'

export function AppHeader() {
  return (
    <header className="sticky top-0 z-10 flex h-14 shrink-0 items-center gap-2 border-b bg-background px-3 md:px-4">
      <SidebarTrigger aria-label="Toggle navigation" />
      <Separator orientation="vertical" className="h-5" />
      <SupervisorPill />
      <div className="ml-2 hidden sm:block">
        <CapMeter />
      </div>
      <div className="ml-auto flex items-center gap-1">
        <CommandPalette />
        <ThemeToggle />
      </div>
    </header>
  )
}
