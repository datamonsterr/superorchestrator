import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { Search } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from '@/components/ui/command'
import { endpoints } from '@/api/endpoints'
import { useSessions } from '@/api/queries'
import { errorMessage } from '@/lib/errors'
import { NAV_ITEMS } from './nav'

function PaletteContent({ close }: { close: () => void }) {
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const sessions = useSessions({}, 20)
  const items = sessions.data?.pages.flatMap((p) => p.items) ?? []
  const go = (to: string) => {
    close()
    void navigate(to)
  }
  async function openRun(id: string) {
    close()
    try {
      const run = await endpoints.getRun(id)
      const sid = run.recent_events[0]?.session_id
      if (sid) void navigate(`/sessions/${sid}/runs/${run.run_id}`)
      else toast.error(`Run ${id} has no session to open`)
    } catch (e) {
      toast.error(`Run ${id}: ${errorMessage(e)}`)
    }
  }
  const trimmed = query.trim()
  return (
    <Command>
      <CommandInput placeholder="Jump to a page, session or run id" value={query} onValueChange={setQuery} />
      <CommandList>
        <CommandEmpty>No results.</CommandEmpty>
        <CommandGroup heading="Pages">
          {NAV_ITEMS.map(({ to, label, icon: Icon }) => (
            <CommandItem key={to} value={`page ${label}`} onSelect={() => go(to)}>
              <Icon aria-hidden /> {label}
            </CommandItem>
          ))}
        </CommandGroup>
        {items.length > 0 && (
          <CommandGroup heading="Sessions">
            {items.map((s) => (
              <CommandItem key={s.id} value={`session ${s.id} ${s.client} ${s.cwd}`} onSelect={() => go(`/sessions/${s.id}`)}>
                {s.client} <span className="text-muted-foreground">{s.cwd}</span>
                <CommandShortcut>{s.status}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {trimmed && (
          <CommandGroup heading="Run" forceMount>
            <CommandItem forceMount value={`open run ${trimmed}`} onSelect={() => void openRun(trimmed)}>
              Open run <code className="font-mono">{trimmed}</code>
            </CommandItem>
          </CommandGroup>
        )}
      </CommandList>
    </Command>
  )
}

/** ctrl/cmd+K. Content mounts only while open so it does not fetch sessions in the background. */
export function CommandPalette() {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        setOpen((o) => !o)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])
  return (
    <>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)} aria-label="Open command palette" className="gap-2">
        <Search aria-hidden />
        <span className="hidden md:inline">Search</span>
        <kbd className="hidden rounded border px-1 font-mono text-[10px] md:inline">Ctrl K</kbd>
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Command palette" description="Jump to a page, a session or a run id">
        {open && <PaletteContent close={() => setOpen(false)} />}
      </CommandDialog>
    </>
  )
}
