import { Controller, useForm, useWatch } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { RelativeTime } from '@/components/common/RelativeTime'
import { useFlagAccount } from '@/api/queries'
import type { AccountView } from '@/api/types'
import { flagSchema, type FlagValues } from './schemas'

function poolsOf(a: AccountView): string[] {
  return [...new Set([...(a.pools ?? []), ...a.headroom.map((h) => h.pool)])]
}

function FlagForm({ account, onClose }: { account: AccountView; onClose: () => void }) {
  const flag = useFlagAccount()
  const pools = poolsOf(account)
  const form = useForm<FlagValues>({ resolver: zodResolver(flagSchema), defaultValues: { pool: 'all', duration: '5', until: '', reason: '' } })
  const { control, register, handleSubmit, formState } = form
  const duration = useWatch({ control, name: 'duration' })
  const flags = account.flags ?? []

  async function onSubmit(v: FlagValues) {
    await flag.mutateAsync({
      name: account.name,
      body: {
        ...(v.pool !== 'all' ? { pool: v.pool } : {}),
        ...(v.duration === 'custom' ? { until: new Date(v.until).toISOString() } : { hours: Number(v.duration) }),
        ...(v.reason ? { reason: v.reason } : {}),
      },
    })
    onClose()
  }
  async function clear(pool?: string) {
    await flag.mutateAsync({ name: account.name, body: { clear: true, ...(pool ? { pool } : {}) } })
    onClose()
  }

  return (
    <form onSubmit={handleSubmit(onSubmit)} noValidate className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>Flag cooldown for {account.name}</DialogTitle>
        <DialogDescription>Routing skips the account (or one pool) until the cooldown ends.</DialogDescription>
      </DialogHeader>
      {flags.length > 0 && (
        <ul className="flex flex-col gap-1 rounded-md border p-2 text-sm" aria-label="Current flags">
          {flags.map((f) => (
            <li key={`${f.pool ?? 'all'}-${f.until}`} className="flex items-center justify-between gap-2">
              <span>
                {f.pool ?? 'all pools'} until <RelativeTime value={f.until} />
                {f.reason ? ` (${f.reason})` : ''}
              </span>
              <Button type="button" variant="ghost" size="sm" disabled={flag.isPending} onClick={() => void clear(f.pool)}>
                Clear
              </Button>
            </li>
          ))}
        </ul>
      )}
      <FieldGroup>
        {pools.length > 0 && (
          <Field>
            <FieldLabel htmlFor="flag-pool">Pool</FieldLabel>
            <Controller
              control={control}
              name="pool"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="flag-pool" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">All pools</SelectItem>
                    {pools.map((p) => (
                      <SelectItem key={p} value={p}>
                        {p}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </Field>
        )}
        <Field>
          <FieldLabel htmlFor="flag-duration">Duration</FieldLabel>
          <Controller
            control={control}
            name="duration"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="flag-duration" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="1">1 hour</SelectItem>
                  <SelectItem value="5">5 hours</SelectItem>
                  <SelectItem value="24">24 hours</SelectItem>
                  <SelectItem value="custom">Until a date…</SelectItem>
                </SelectContent>
              </Select>
            )}
          />
        </Field>
        {duration === 'custom' && (
          <Field data-invalid={!!formState.errors.until}>
            <FieldLabel htmlFor="flag-until">Until</FieldLabel>
            <Input id="flag-until" type="datetime-local" aria-invalid={!!formState.errors.until} {...register('until')} />
            <FieldError errors={[formState.errors.until]} />
          </Field>
        )}
        <Field>
          <FieldLabel htmlFor="flag-reason">Reason (optional)</FieldLabel>
          <Input id="flag-reason" autoComplete="off" placeholder="quota" {...register('reason')} />
        </Field>
      </FieldGroup>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" disabled={flag.isPending}>
          Flag account
        </Button>
      </DialogFooter>
    </form>
  )
}

export function FlagDialog({ account, onClose }: { account: AccountView | null; onClose: () => void }) {
  return (
    <Dialog open={account !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{account && <FlagForm key={account.name} account={account} onClose={onClose} />}</DialogContent>
    </Dialog>
  )
}
