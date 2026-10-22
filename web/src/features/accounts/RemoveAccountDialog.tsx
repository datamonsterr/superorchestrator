import { useState } from 'react'
import { toast } from 'sonner'
import { Checkbox } from '@/components/ui/checkbox'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { useRemoveAccount } from '@/api/queries'

export function RemoveAccountDialog({ name, onClose }: { name: string | null; onClose: () => void }) {
  const remove = useRemoveAccount()
  const [purge, setPurge] = useState(false)
  return (
    <ConfirmDialog
      key={name ?? 'none'}
      open={name !== null}
      onOpenChange={(o) => {
        if (!o) {
          setPurge(false)
          onClose()
        }
      }}
      title={`Remove ${name ?? 'account'}?`}
      destructive
      confirmLabel="Remove account"
      requireText={name ?? ''}
      pending={remove.isPending}
      description={
        <span className="flex flex-col gap-3">
          <span>The account is unregistered and no longer used for routing.</span>
          <span className="flex items-start gap-2">
            <Checkbox id="remove-purge" checked={purge} onCheckedChange={(v) => setPurge(v === true)} />
            <label htmlFor="remove-purge" className="text-sm">
              Also delete the credential home
              {purge && <strong className="block text-destructive">This permanently deletes its stored sign-in. It cannot be undone.</strong>}
            </label>
          </span>
        </span>
      }
      onConfirm={() => {
        if (!name) return
        remove.mutate(
          { name, purge },
          {
            onSuccess: () => {
              toast.success(`Removed ${name}`)
              setPurge(false)
              onClose()
            },
          },
        )
      }}
    />
  )
}
