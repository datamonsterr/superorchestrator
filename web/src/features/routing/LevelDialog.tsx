import { useMemo } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { errorMessage } from '@/lib/errors'
import type { Level, LevelView } from '@/api/types'
import { fromLevel, levelOf, levelSchema, NO_TIER, TIERS, toLevel, type LevelValues } from './schemas'

interface Props {
  /** `null` = closed, `'new'` = add, otherwise the level being edited. */
  target: LevelView | 'new' | null
  levels: LevelView[]
  onClose: () => void
  /** Persist the full next list; rejects on failure so the dialog stays open. */
  onSave: (next: Level[]) => Promise<unknown>
}

function LevelForm({ target, levels, onClose, onSave }: Omit<Props, 'target'> & { target: LevelView | 'new' }) {
  const editing = target === 'new' ? null : target
  const schema = useMemo(
    () => levelSchema(levels.filter((l) => l.name !== editing?.name).map((l) => l.name), (editing?.choices.length ?? 0) > 0),
    [levels, editing],
  )
  const form = useForm<LevelValues>({ resolver: zodResolver(schema), defaultValues: fromLevel(editing) })
  const { control, register, handleSubmit, setError, formState } = form
  const err = formState.errors

  async function submit(v: LevelValues) {
    const next = toLevel(v, editing ? levelOf(editing).choices ?? [] : [])
    const list = editing ? levels.map((l) => (l.name === editing.name ? next : levelOf(l))) : [...levels.map(levelOf), next]
    try {
      await onSave(list)
      onClose()
    } catch (e) {
      setError('root', { type: 'server', message: errorMessage(e) })
    }
  }

  return (
    <form onSubmit={handleSubmit(submit)} noValidate className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>{editing ? `Edit ${editing.name}` : 'Add level'}</DialogTitle>
        <DialogDescription>A level is a range of complexity and the models allowed for it.</DialogDescription>
      </DialogHeader>
      <FieldGroup>
        <Field data-invalid={!!err.name}>
          <FieldLabel htmlFor="level-name">Name</FieldLabel>
          <Input id="level-name" autoComplete="off" aria-invalid={!!err.name} {...register('name')} />
          <FieldError errors={[err.name]} />
        </Field>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field data-invalid={!!err.min}>
            <FieldLabel htmlFor="level-min">Minimum</FieldLabel>
            <Input id="level-min" inputMode="numeric" autoComplete="off" aria-invalid={!!err.min} {...register('min')} />
            <FieldError errors={[err.min]} />
          </Field>
          <Field data-invalid={!!err.max}>
            <FieldLabel htmlFor="level-max">Maximum</FieldLabel>
            <Input id="level-max" inputMode="numeric" autoComplete="off" aria-invalid={!!err.max} {...register('max')} />
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
          <FieldDescription>Used only while the level lists no models: any eligible model of this class.</FieldDescription>
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
