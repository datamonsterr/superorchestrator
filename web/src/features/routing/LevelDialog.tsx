import { useMemo } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { applyApiFieldErrors } from '@/lib/form-errors'
import { errorMessage } from '@/lib/errors'
import type { Level } from '@/api/types'
import { levelSchema, NO_TIER, TIERS, type LevelValues } from './schemas'

interface Props {
  /** `null` = closed, `'new'` = add, otherwise the level being edited. */
  target: Level | 'new' | null
  levels: Level[]
  onClose: () => void
  /** Persists the full next list; rejects on failure so the dialog stays open. */
  onSave: (next: Level[]) => Promise<unknown>
}

const FIELDS = ['name', 'min', 'max', 'tier'] as const

function LevelForm({ target, levels, onClose, onSave }: Omit<Props, 'target'> & { target: Level | 'new' }) {
  const editing = target === 'new' ? null : target
  const others = useMemo(() => levels.filter((l) => l.name !== editing?.name), [levels, editing])
  const schema = useMemo(() => levelSchema(others), [others])
  const { control, register, handleSubmit, setError, formState } = useForm<LevelValues>({
    resolver: zodResolver(schema),
    defaultValues: editing
      ? { name: editing.name, min: editing.min, max: editing.max, tier: editing.tier ?? NO_TIER }
      : { name: '', min: Math.min(100, Math.max(-1, ...levels.map((l) => l.max)) + 1), max: 100, tier: NO_TIER },
  })
  const err = formState.errors

  async function submit(v: LevelValues) {
    const models = editing?.models ?? []
    if (models.length === 0 && v.tier === NO_TIER) {
      setError('tier', { type: 'validate', message: 'A level without models needs a tier' })
      return
    }
    const next: Level = { name: v.name, min: v.min, max: v.max, models, ...(v.tier !== NO_TIER ? { tier: v.tier as Level['tier'] } : {}) }
    const list = editing ? levels.map((l) => (l.name === editing.name ? next : l)) : [...levels, next]
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
        <DialogTitle>{editing ? `Edit level ${editing.name}` : 'Add level'}</DialogTitle>
        <DialogDescription>A level is a named range of task complexity (0 to 100) with the models the router may pick for it.</DialogDescription>
      </DialogHeader>
      <FieldGroup>
        <Field data-invalid={!!err.name}>
          <FieldLabel htmlFor="level-name">Name</FieldLabel>
          <Input id="level-name" autoComplete="off" aria-invalid={!!err.name} {...register('name')} />
          <FieldError errors={[err.name]} />
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field data-invalid={!!err.min}>
            <FieldLabel htmlFor="level-min">From</FieldLabel>
            <Input id="level-min" type="number" min={0} max={100} inputMode="numeric" aria-invalid={!!err.min} {...register('min', { valueAsNumber: true })} />
            <FieldError errors={[err.min]} />
          </Field>
          <Field data-invalid={!!err.max}>
            <FieldLabel htmlFor="level-max">To</FieldLabel>
            <Input id="level-max" type="number" min={0} max={100} inputMode="numeric" aria-invalid={!!err.max} {...register('max', { valueAsNumber: true })} />
            <FieldError errors={[err.max]} />
          </Field>
        </div>
        <Field data-invalid={!!err.tier}>
          <FieldLabel htmlFor="level-tier">Tier (optional)</FieldLabel>
          <Controller
            control={control}
            name="tier"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="level-tier" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NO_TIER}>None</SelectItem>
                  {TIERS.map((t) => (
                    <SelectItem key={t} value={t}>
                      {t}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          <FieldDescription>Used only when the level lists no models.</FieldDescription>
          <FieldError errors={[err.tier]} />
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
          {formState.isSubmitting ? 'Saving…' : 'Save level'}
        </Button>
      </DialogFooter>
    </form>
  )
}

export function LevelDialog(props: Props) {
  const { target, onClose } = props
  return (
    <Dialog open={target !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{target && <LevelForm key={target === 'new' ? 'new' : target.name} {...props} target={target} />}</DialogContent>
    </Dialog>
  )
}
