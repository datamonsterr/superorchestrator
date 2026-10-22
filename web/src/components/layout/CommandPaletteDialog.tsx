import { useState } from 'react'
import { useNavigate } from 'react-router'
import { toast } from 'sonner'
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

/** The dialog and its data hooks; lazy-loaded by CommandPalette so cmdk stays out of the entry chunk. */
export default function CommandPaletteDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Command palette" description="Jump to a page, a session or a run id">
      {open && <PaletteContent close={() => onOpenChange(false)} />}
    </CommandDialog>
  )
}
