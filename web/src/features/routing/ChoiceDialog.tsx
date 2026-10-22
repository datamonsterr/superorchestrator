import { useMemo } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useModels } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import { errorMessage } from '@/lib/errors'
import type { Choice } from '@/api/types'
import { choiceSchema, fromChoice, HARNESSES, toChoice, type ChoiceValues } from './schemas'

interface Props {
  /** `null` = closed, `'new'` = add, otherwise the choice being edited. */
  target: Choice | 'new' | null
  choices: Choice[]
  onClose: () => void
  /** Persist the full next list; rejects on failure so the dialog stays open. */
  onSave: (next: Choice[]) => Promise<unknown>
}

const EMPTY: ChoiceValues = { id: '', harness: 'codex', model: '', effort: '', roles: 'worker', description: '', enabled: true }
const FIELDS = ['id', 'harness', 'model', 'effort', 'roles', 'description', 'enabled'] as const

function ChoiceForm({ target, choices, onClose, onSave }: Omit<Props, 'target'> & { target: Choice | 'new' }) {
  const editing = target === 'new' ? null : target
  const models = useModels()
  const schema = useMemo(() => choiceSchema(choices.filter((c) => c.id !== editing?.id).map((c) => c.id)), [choices, editing])
  const form = useForm<ChoiceValues>({ resolver: zodResolver(schema), defaultValues: editing ? fromChoice(editing) : EMPTY })
  const { control, register, handleSubmit, setError, formState } = form
  const harness = useWatch({ control, name: 'harness' })
  const model = useWatch({ control, name: 'model' })
  const catalog = (models.data?.models ?? []).filter((m) => m.harness === harness)
  const efforts = catalog.find((m) => m.id === model)?.efforts ?? []
  const err = formState.errors

  async function submit(v: ChoiceValues) {
    const next = toChoice(v)
    const list = editing ? choices.map((c) => (c.id === editing.id ? next : c)) : [...choices, next]
    try {
      await onSave(list)
      onClose()
    } catch (e) {
      if (!applyApiFieldErrors(e, setError, FIELDS)) setError('root', { type: 'server', message: errorMessage(e) })
    }
  }

  return (
    <form onSubmit={handleSubmit(submit)} noValidate className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>{editing ? `Edit ${editing.id}` : 'Add choice'}</DialogTitle>
        <DialogDescription>A choice is a harness, model and effort the router may pick.</DialogDescription>
      </DialogHeader>
      <FieldGroup>
        <Field data-invalid={!!err.id}>
          <FieldLabel htmlFor="choice-id">Id</FieldLabel>
          <Input id="choice-id" autoComplete="off" disabled={!!editing} aria-invalid={!!err.id} {...register('id')} />
          <FieldError errors={[err.id]} />
        </Field>
        <Field data-invalid={!!err.harness}>
          <FieldLabel htmlFor="choice-harness">Harness</FieldLabel>
          <Controller
            control={control}
            name="harness"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="choice-harness" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {HARNESSES.map((h) => (
                    <SelectItem key={h} value={h}>
                      {h}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          <FieldError errors={[err.harness]} />
        </Field>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field data-invalid={!!err.model}>
            <FieldLabel htmlFor="choice-model">Model</FieldLabel>
            <Input id="choice-model" list="choice-models" autoComplete="off" aria-invalid={!!err.model} {...register('model')} />
            <datalist id="choice-models">
              {catalog.map((m) => (
                <option key={m.id} value={m.id} />
              ))}
            </datalist>
            <FieldError errors={[err.model]} />
          </Field>
          <Field data-invalid={!!err.effort}>
            <FieldLabel htmlFor="choice-effort">Effort (optional)</FieldLabel>
            <Input id="choice-effort" list="choice-efforts" autoComplete="off" {...register('effort')} />
            <datalist id="choice-efforts">
              {efforts.map((e) => (
                <option key={e} value={e} />
              ))}
            </datalist>
            <FieldError errors={[err.effort]} />
          </Field>
        </div>
        <Field data-invalid={!!err.roles}>
          <FieldLabel htmlFor="choice-roles">Roles</FieldLabel>
          <Input id="choice-roles" autoComplete="off" aria-invalid={!!err.roles} {...register('roles')} />
          <FieldDescription>Comma separated. A route without a role asks for “worker”.</FieldDescription>
          <FieldError errors={[err.roles]} />
        </Field>
        <Field data-invalid={!!err.description}>
          <FieldLabel htmlFor="choice-description">Description</FieldLabel>
          <Textarea id="choice-description" rows={3} {...register('description')} />
          <FieldDescription>Read by the router: say what this choice is best at.</FieldDescription>
          <FieldError errors={[err.description]} />
        </Field>
        <Field orientation="horizontal">
          <Controller control={control} name="enabled" render={({ field }) => <Switch id="choice-enabled" checked={field.value} onCheckedChange={field.onChange} />} />
          <FieldLabel htmlFor="choice-enabled">Enabled</FieldLabel>
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
          {formState.isSubmitting ? 'Saving…' : 'Save choice'}
        </Button>
      </DialogFooter>
    </form>
  )
}

export function ChoiceDialog(props: Props) {
  const { target, onClose } = props
  return (
    <Dialog open={target !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{target && <ChoiceForm key={target === 'new' ? 'new' : target.id} {...props} target={target} />}</DialogContent>
    </Dialog>
  )
}
