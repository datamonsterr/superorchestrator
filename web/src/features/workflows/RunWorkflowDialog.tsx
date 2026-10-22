import { useState } from 'react'
import { useNavigate } from 'react-router'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { useRunWorkflow } from '@/api/queries'

function Body({ name, initial, onClose }: { name: string; initial: string; onClose: () => void }) {
  const run = useRunWorkflow()
  const navigate = useNavigate()
  const [text, setText] = useState(initial)
  let parsed: unknown
  let jsonError: string | null = null
  if (text.trim() !== '') {
    try {
      parsed = JSON.parse(text)
    } catch (e) {
      jsonError = e instanceof Error ? e.message : 'Invalid JSON'
    }
  }
  return (
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Run {name}</DialogTitle>
        <DialogDescription>Optional arguments, as JSON. Leave empty to run without arguments.</DialogDescription>
      </DialogHeader>
      <label className="flex flex-col gap-1.5 text-sm">
        <span className="font-medium">Arguments</span>
        <Textarea value={text} onChange={(e) => setText(e.target.value)} className="min-h-32 font-mono text-xs" spellCheck={false} aria-invalid={!!jsonError} />
        {jsonError && (
          <span role="alert" className="text-xs text-destructive">
            {jsonError}
          </span>
        )}
      </label>
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button
          disabled={!!jsonError || run.isPending}
          // A failed start (including a 501 from a supervisor without workflows) toasts through the
          // MutationCache.onError handler in lib/query-client.ts, so nothing is swallowed here.
          onClick={() =>
            run.mutate(
              { name, args: parsed },
              {
                onSuccess: (r) => {
                  toast.success(`Started ${name}`)
                  onClose()
                  void navigate(`/workflows/${encodeURIComponent(r.id)}`)
                },
              },
            )
          }
        >
          {run.isPending ? 'Starting...' : 'Run'}
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

/** `name` null keeps the dialog closed. The body remounts per open so the text resets. */
export function RunWorkflowDialog({ name, onClose, initialArgs = '' }: { name: string | null; onClose: () => void; initialArgs?: string }) {
  return (
    <Dialog open={name !== null} onOpenChange={(o) => !o && onClose()}>
      {name !== null && <Body name={name} initial={initialArgs} onClose={onClose} />}
    </Dialog>
  )
}
