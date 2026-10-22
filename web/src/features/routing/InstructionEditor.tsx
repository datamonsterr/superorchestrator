import { useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Textarea } from '@/components/ui/textarea'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { usePutChoices } from '@/api/queries'
import type { ChoicesView } from '@/api/types'

/** Edits `routing_instruction`; the choices are sent back unchanged. */
export function InstructionEditor({ view }: { view: ChoicesView }) {
  const put = usePutChoices()
  const [draft, setDraft] = useState<string | null>(null)
  const value = draft ?? view.routing_instruction
  const dirty = draft !== null && draft !== view.routing_instruction

  async function save() {
    try {
      await put.mutateAsync({ choices: view.choices, routing_instruction: value })
      setDraft(null)
      toast.success('Routing instruction saved')
    } catch {
      // The global mutation handler toasts; the alert below keeps the message in place.
    }
  }

  return (
    <div className="flex max-w-3xl flex-col gap-3">
      <Field>
        <FieldLabel htmlFor="routing-instruction">Routing instruction</FieldLabel>
        <Textarea id="routing-instruction" rows={10} className="font-mono text-sm" value={value} onChange={(e) => setDraft(e.target.value)} />
        <FieldDescription>
          <span aria-live="polite">{value.length.toLocaleString()} characters</span>. Free text the router reads on every route.
        </FieldDescription>
      </Field>
      {put.isError && <ErrorAlert error={put.error} title="Could not save the instruction" />}
      <div className="flex gap-2">
        <Button disabled={!dirty || put.isPending} onClick={() => void save()}>
          {put.isPending ? 'Saving…' : 'Save'}
        </Button>
        <Button variant="outline" disabled={!dirty || put.isPending} onClick={() => setDraft(null)}>
          Revert
        </Button>
      </div>
    </div>
  )
}
