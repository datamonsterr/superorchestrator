import { useState } from 'react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { useRemoveAccount } from '@/api/queries'

/**
 * Own alert dialog rather than ConfirmDialog: the purge checkbox is a control and
 * must not live inside the description paragraph.
 */
function RemoveBody({ name, open, onClose }: { name: string; open: boolean; onClose: () => void }) {
  const remove = useRemoveAccount()
  const [typed, setTyped] = useState('')
  const [purge, setPurge] = useState(false)
  return (
    <AlertDialog open={open} onOpenChange={(o) => !o && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Remove {name}?</AlertDialogTitle>
          <AlertDialogDescription>The account is unregistered and no longer used for routing.</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex items-start gap-2">
          <Checkbox id="remove-purge" checked={purge} onCheckedChange={(v) => setPurge(v === true)} />
          <label htmlFor="remove-purge" className="text-sm">
            Also delete the credential home
            {purge && <strong className="block text-destructive">This permanently deletes its stored sign-in. It cannot be undone.</strong>}
          </label>
        </div>
        <label className="flex flex-col gap-1.5 text-sm">
          <span>
            Type <strong className="font-mono">{name}</strong> to confirm
          </span>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" />
        </label>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button
            variant="destructive"
            disabled={remove.isPending || typed !== name}
            onClick={() =>
              remove.mutate(
                { name, purge },
                {
                  onSuccess: () => {
                    toast.success(`Removed ${name}`)
                    onClose()
                  },
                },
              )
            }
          >
            Remove account
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

export function RemoveAccountDialog({ name, onClose }: { name: string | null; onClose: () => void }) {
  // Keep the last name mounted while closing so the exit animation has content; state resets per account.
  const [last, setLast] = useState(name)
  if (name !== null && name !== last) setLast(name)
  return last ? <RemoveBody key={last} name={last} open={name !== null} onClose={onClose} /> : null
}
