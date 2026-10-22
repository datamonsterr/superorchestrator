import { useState } from 'react'
import { toast } from 'sonner'
import { useAcceptRun, useCancelRun, useRejectRun, useSendMessage } from '@/api/queries'
import type { MessageKind, RunState } from '@/api/generated'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { isTerminal } from './data'

/** Footer actions, enabled by state (design 4.9). Failures toast through the global mutation handler. */
export function RunActions({ runId, state }: { runId: string; state: RunState }) {
  const send = useSendMessage()
  const cancel = useCancelRun()
  const accept = useAcceptRun()
  const reject = useRejectRun()
  const [composing, setComposing] = useState(false)
  const [text, setText] = useState('')
  const [kind, setKind] = useState<Extract<MessageKind, 'message' | 'nudge'>>('message')
  const [confirmCancel, setConfirmCancel] = useState(false)
  const [rejecting, setRejecting] = useState(false)
  const [feedback, setFeedback] = useState('')

  const terminal = isTerminal(state)
  const review = state === 'pending_review'
  const busy = send.isPending || cancel.isPending || accept.isPending || reject.isPending

  async function doSend() {
    await send.mutateAsync({ id: runId, body: { text: text.trim(), kind } })
    toast.success(terminal ? 'Message sent, resuming the run' : 'Message sent')
    setText('')
    setComposing(false)
  }
  async function doCancel() {
    await cancel.mutateAsync({ id: runId })
    toast.success('Run cancelled')
    setConfirmCancel(false)
  }
  async function doAccept() {
    await accept.mutateAsync({ id: runId })
    toast.success('Run accepted')
  }
  async function doReject() {
    await reject.mutateAsync({ id: runId, feedback: feedback.trim() })
    toast.success('Run rejected, feedback sent')
    setFeedback('')
    setRejecting(false)
  }
  // The global handler toasts the failure; keep dialogs open for a retry.
  const swallow = (p: Promise<void>) => void p.catch(() => {})

  return (
    <div className="flex flex-col gap-2 border-t p-3">
      {composing && (
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (text.trim()) swallow(doSend())
          }}
        >
          <Textarea aria-label="Message" placeholder="Message to the run" value={text} onChange={(e) => setText(e.target.value)} rows={3} autoFocus />
          <div className="flex items-center gap-2">
            <Select value={kind} onValueChange={(v) => setKind(v as typeof kind)}>
              <SelectTrigger size="sm" aria-label="Message kind" className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="message">Message</SelectItem>
                <SelectItem value="nudge">Nudge</SelectItem>
              </SelectContent>
            </Select>
            <Button type="button" variant="ghost" size="sm" className="ml-auto" onClick={() => setComposing(false)}>
              Discard
            </Button>
            <Button type="submit" size="sm" disabled={!text.trim() || send.isPending}>
              {terminal ? 'Send and resume' : 'Send'}
            </Button>
          </div>
        </form>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {!composing && (
          <Button type="button" variant="outline" size="sm" onClick={() => setComposing(true)}>
            {terminal ? 'Send and resume' : 'Send message'}
          </Button>
        )}
        <Button type="button" variant="outline" size="sm" disabled={terminal || busy} onClick={() => setConfirmCancel(true)}>
          Cancel run
        </Button>
        {review && (
          <>
            <Button type="button" size="sm" className="ml-auto" disabled={busy} onClick={() => swallow(doAccept())}>
              Accept
            </Button>
            <Button type="button" variant="destructive" size="sm" disabled={busy} onClick={() => setRejecting(true)}>
              Reject
            </Button>
          </>
        )}
      </div>

      <ConfirmDialog
        open={confirmCancel}
        onOpenChange={setConfirmCancel}
        title="Cancel this run?"
        description="This cancels the run and its subtree."
        confirmLabel="Cancel run"
        destructive
        pending={cancel.isPending}
        onConfirm={() => swallow(doCancel())}
      />

      <Dialog open={rejecting} onOpenChange={setRejecting}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Reject this run</DialogTitle>
            <DialogDescription>The feedback is sent to the run so it can try again.</DialogDescription>
          </DialogHeader>
          <Textarea aria-label="Feedback" placeholder="What should change?" value={feedback} onChange={(e) => setFeedback(e.target.value)} rows={4} />
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setRejecting(false)}>
              Back
            </Button>
            <Button type="button" variant="destructive" disabled={!feedback.trim() || reject.isPending} onClick={() => swallow(doReject())}>
              Reject run
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
