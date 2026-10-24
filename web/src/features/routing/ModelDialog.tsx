import { useMemo } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useModels } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import { errorMessage } from '@/lib/errors'
import type { Level } from '@/api/types'
import { buildRef, HARNESSES, modelRefSchema, type ModelRefValues } from './schemas'

interface Props {
  /** The level a model is added to; `null` = closed. */
  level: Level | null
  onClose: () => void
  /** Persists the level's next model list; rejects on failure so the dialog stays open. */
  onSave: (level: Level, ref: string) => Promise<unknown>
}

const EMPTY: ModelRefValues = { harness: 'codex', model: '', effort: '' }
const FIELDS = ['harness', 'model', 'effort'] as const

function ModelForm({ level, onClose, onSave }: Omit<Props, 'level'> & { level: Level }) {
  const models = useModels()
  const schema = useMemo(() => modelRefSchema(level.models), [level.models])
  const { control, register, handleSubmit, setError, formState } = useForm<ModelRefValues>({ resolver: zodResolver(schema), defaultValues: EMPTY })
  const harness = useWatch({ control, name: 'harness' })
  const model = useWatch({ control, name: 'model' })
  const effort = useWatch({ control, name: 'effort' })
  const catalog = (models.data?.models ?? []).filter((m) => m.harness === harness)
  const efforts = catalog.find((m) => m.id === model.trim())?.efforts ?? []
  const err = formState.errors
  const preview = buildRef(harness, model || '<model>', effort)

  async function submit(v: ModelRefValues) {
    try {
      await onSave(level, buildRef(v.harness, v.model, v.effort))
      onClose()
    } catch (e) {
      if (!applyApiFieldErrors(e, setError, FIELDS)) setError('root', { type: 'server', message: errorMessage(e) })
    }
  }

  return (
    <form onSubmit={handleSubmit(submit)} noValidate className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>Add model to {level.name}</DialogTitle>
        <DialogDescription>The model goes last in the level, so it is the least preferred. Move it up afterwards.</DialogDescription>
      </DialogHeader>
      <FieldGroup>
        <Field data-invalid={!!err.harness}>
          <FieldLabel htmlFor="model-harness">Harness</FieldLabel>
          <Controller
            control={control}
            name="harness"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="model-harness" className="w-full">
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
            <FieldLabel htmlFor="model-name">Model</FieldLabel>
            <Input id="model-name" list="model-names" autoComplete="off" aria-invalid={!!err.model} {...register('model')} />
            <datalist id="model-names">
              {catalog.map((m) => (
                <option key={m.id} value={m.id} />
              ))}
            </datalist>
            <FieldError errors={[err.model]} />
          </Field>
          <Field data-invalid={!!err.effort}>
            <FieldLabel htmlFor="model-effort">Effort (optional)</FieldLabel>
            <Input id="model-effort" list="model-efforts" autoComplete="off" {...register('effort')} />
            <datalist id="model-efforts">
              {efforts.map((e) => (
                <option key={e} value={e} />
              ))}
            </datalist>
            <FieldError errors={[err.effort]} />
          </Field>
        </div>
        <Field>
          <FieldLabel htmlFor="model-preview">Resulting ref</FieldLabel>
          <output id="model-preview" className="block rounded-md bg-muted px-2 py-1.5 font-mono text-xs break-all">
            {preview}
          </output>
          <FieldDescription>Written as harness/model/effort; the effort is left out when empty.</FieldDescription>
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
          {formState.isSubmitting ? 'Saving…' : 'Save model'}
        </Button>
      </DialogFooter>
    </form>
  )
}

export function ModelDialog(props: Props) {
  const { level, onClose } = props
  return (
    <Dialog open={level !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{level && <ModelForm key={level.name} {...props} level={level} />}</DialogContent>
    </Dialog>
  )
}
