import { Controller, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel, FieldSet, FieldLegend } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { usePatchAccount } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import type { AccountView } from '@/api/types'
import { AGY_POOLS, editAccountSchema, type EditAccountValues } from './schemas'

function EditForm({ account, onClose }: { account: AccountView; onClose: () => void }) {
  const patch = usePatchAccount()
  const form = useForm<EditAccountValues>({
    resolver: zodResolver(editAccountSchema),
    defaultValues: { cap: account.cap, priority: account.priority ?? 0, enabled: account.enabled, pools: account.pools ?? [] },
  })
  const { control, register, handleSubmit, setError, formState } = form
  async function onSubmit(v: EditAccountValues) {
    try {
      await patch.mutateAsync({ name: account.name, body: { cap: v.cap, ...(formState.dirtyFields.priority ? { priority: v.priority } : {}), enabled: v.enabled, ...(account.harness === 'agy' ? { pools: v.pools } : {}) } })
      onClose()
    } catch (e) {
      applyApiFieldErrors(e, setError, ['cap', 'priority', 'enabled', 'pools'])
    }
  }
  return (
    <form onSubmit={handleSubmit(onSubmit)} noValidate className="flex flex-1 flex-col gap-4 overflow-y-auto px-4">
      <FieldGroup>
        <Field data-invalid={!!formState.errors.cap}>
          <FieldLabel htmlFor="edit-cap">Concurrency cap</FieldLabel>
          <Input id="edit-cap" type="number" min={1} max={16} inputMode="numeric" aria-invalid={!!formState.errors.cap} {...register('cap', { valueAsNumber: true })} />
          <FieldDescription>Runs allowed at once on this account (1 to 16).</FieldDescription>
          <FieldError errors={[formState.errors.cap]} />
        </Field>
        <Field data-invalid={!!formState.errors.priority}>
          <FieldLabel htmlFor="edit-priority">Priority</FieldLabel>
          <Input id="edit-priority" type="number" inputMode="numeric" aria-invalid={!!formState.errors.priority} {...register('priority', { valueAsNumber: true })} />
          <FieldDescription>Lower numbers are tried first.</FieldDescription>
          <FieldError errors={[formState.errors.priority]} />
        </Field>
        {account.harness === 'agy' && (
          <FieldSet>
            <FieldLegend variant="label">Pools</FieldLegend>
            <Controller
              control={control}
              name="pools"
              render={({ field }) => (
                <div className="flex flex-col gap-2">
                  {AGY_POOLS.map((p) => (
                    <Field key={p} orientation="horizontal">
                      <Checkbox
                        id={`pool-${p}`}
                        checked={field.value.includes(p)}
                        onCheckedChange={(on) => field.onChange(on === true ? [...field.value, p] : field.value.filter((x) => x !== p))}
                      />
                      <FieldLabel htmlFor={`pool-${p}`}>{p}</FieldLabel>
                    </Field>
                  ))}
                </div>
              )}
            />
          </FieldSet>
        )}
        <Field orientation="horizontal">
          <Controller control={control} name="enabled" render={({ field }) => <Switch id="edit-enabled" checked={field.value} onCheckedChange={field.onChange} />} />
          <FieldLabel htmlFor="edit-enabled">Enabled</FieldLabel>
        </Field>
      </FieldGroup>
      <SheetFooter className="px-0">
        <Button type="submit" disabled={patch.isPending}>
          {patch.isPending ? 'Saving…' : 'Save'}
        </Button>
        <Button type="button" variant="outline" onClick={onClose}>
          Cancel
        </Button>
      </SheetFooter>
    </form>
  )
}

export function EditAccountSheet({ account, onClose }: { account: AccountView | null; onClose: () => void }) {
  return (
    <Sheet open={account !== null} onOpenChange={(o) => !o && onClose()}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>Edit {account?.name}</SheetTitle>
          <SheetDescription>Cap, priority, pools and whether the account is used.</SheetDescription>
        </SheetHeader>
        {account && <EditForm key={account.name} account={account} onClose={onClose} />}
      </SheetContent>
    </Sheet>
  )
}
