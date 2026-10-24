import { Controller, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { errorMessage } from '@/lib/errors'
import type { Choice, LevelView } from '@/api/types'
import { addModelSchema, choiceLabel, type AddModelValues } from './schemas'

interface Props {
  /** The level to add to; `null` = closed. */
  level: LevelView | null
  choices: Choice[]
  onClose: () => void
  /** Persist the model; rejects on failure so the dialog stays open. */
  onSave: (level: LevelView, id: string, priority: number | undefined) => Promise<unknown>
}

function AddModelForm({ level, choices, onClose, onSave }: Omit<Props, 'level'> & { level: LevelView }) {
  const free = choices.filter((c) => !level.choices.some((x) => x.id === c.id))
  const form = useForm<AddModelValues>({ resolver: zodResolver(addModelSchema), defaultValues: { choice: '', priority: '' } })
  const { control, register, handleSubmit, setError, formState } = form
  const err = formState.errors

  async function submit(v: AddModelValues) {
    try {
      await onSave(level, v.choice, v.priority === '' ? undefined : Number(v.priority))
      onClose()
    } catch (e) {
      setError('root', { type: 'server', message: errorMessage(e) })
    }
  }

  return (
    <form onSubmit={handleSubmit(submit)} noValidate className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>{`Add model to ${level.name}`}</DialogTitle>
        <DialogDescription>Pick an existing choice. A lower priority is preferred.</DialogDescription>
      </DialogHeader>
      <FieldGroup>
        <Field data-invalid={!!err.choice}>
          <FieldLabel htmlFor="add-model-choice">Choice</FieldLabel>
          <Controller
            control={control}
            name="choice"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="add-model-choice" className="w-full" aria-invalid={!!err.choice}>
                  <SelectValue placeholder={free.length ? 'Pick a choice' : 'Every choice is already in this level'} />
                </SelectTrigger>
                <SelectContent>
                  {free.map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {choiceLabel(c)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          <FieldError errors={[err.choice]} />
        </Field>
        <Field data-invalid={!!err.priority}>
          <FieldLabel htmlFor="add-model-priority">Priority (optional)</FieldLabel>
          <Input id="add-model-priority" inputMode="numeric" autoComplete="off" aria-invalid={!!err.priority} {...register('priority')} />
          <FieldDescription>Leave empty to list the model after those with a priority.</FieldDescription>
          <FieldError errors={[err.priority]} />
        </Field>
      </FieldGroup>
      {err.root && (
        <p role="alert" className="text-sm text-destructive">
          {err.root.message}
        </p>
      )}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" disabled={formState.isSubmitting}>
          {formState.isSubmitting ? 'Saving…' : 'Add model'}
        </Button>
      </DialogFooter>
    </form>
  )
}

export function AddModelDialog(props: Props) {
  const { level, onClose } = props
  return (
    <Dialog open={level !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{level && <AddModelForm key={level.name} {...props} level={level} />}</DialogContent>
    </Dialog>
  )
}
