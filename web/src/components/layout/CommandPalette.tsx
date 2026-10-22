import { lazy, Suspense, useEffect, useState } from 'react'
import { Search } from 'lucide-react'
import { Button } from '@/components/ui/button'

const Dialog = lazy(() => import('./CommandPaletteDialog'))

/** ctrl/cmd+K. The dialog loads on first open, so cmdk and the session query stay out of the entry bundle. */
export function CommandPalette() {
  const [open, setOpen] = useState(false)
  const [loaded, setLoaded] = useState(false)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        setLoaded(true)
        setOpen((o) => !o)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setLoaded(true)
          setOpen(true)
        }}
        aria-label="Open command palette"
        className="gap-2"
      >
        <Search aria-hidden />
        <span className="hidden md:inline">Search</span>
        <kbd className="hidden rounded border px-1 font-mono text-[10px] md:inline">Ctrl K</kbd>
      </Button>
      {loaded && (
        <Suspense fallback={null}>
          <Dialog open={open} onOpenChange={setOpen} />
        </Suspense>
      )}
    </>
  )
}
