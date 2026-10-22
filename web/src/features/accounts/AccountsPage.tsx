import { useState } from 'react'
import { Plus, ScanSearch, Users } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { PageHeader } from '@/components/layout/PageHeader'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { useAccounts, usePatchAccount } from '@/api/queries'
import type { AccountView } from '@/api/types'
import { AccountsTable } from './AccountsTable'
import { AddAccountDialog } from './AddAccountDialog'
import { EditAccountSheet } from './EditAccountSheet'
import { FlagDialog } from './FlagDialog'
import { ImportDialog } from './ImportDialog'
import { RemoveAccountDialog } from './RemoveAccountDialog'
import { SignInDialog } from './SignInDialog'

export function AccountsPage() {
  const query = useAccounts()
  const patch = usePatchAccount()
  const [adding, setAdding] = useState(false)
  const [importing, setImporting] = useState(false)
  const [signIn, setSignIn] = useState<string | null>(null)
  const [editing, setEditing] = useState<string | null>(null)
  const [flagging, setFlagging] = useState<string | null>(null)
  const [removing, setRemoving] = useState<string | null>(null)
  const accounts = query.data ?? []
  const byName = (n: string | null): AccountView | null => accounts.find((a) => a.name === n) ?? null

  return (
    <>
      <PageHeader
        title="Accounts"
        description="Manage accounts and sign-in."
        actions={
          <>
            <Button variant="outline" onClick={() => setImporting(true)}>
              <ScanSearch aria-hidden /> Scan and import
            </Button>
            <Button onClick={() => setAdding(true)}>
              <Plus aria-hidden /> Add account
            </Button>
          </>
        }
      />
      {query.isError && !query.data ? (
        <ErrorAlert error={query.error} title="Could not load accounts" onRetry={() => void query.refetch()} />
      ) : query.isPending ? (
        <TableSkeleton rows={4} columns={7} />
      ) : accounts.length === 0 ? (
        <EmptyState
          icon={Users}
          title="Add your first account"
          description="Accounts are the logins that runs execute under."
          action={<Button onClick={() => setAdding(true)}>Add account</Button>}
        />
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <AccountsTable
            accounts={accounts}
            actions={{
              pending: patch.isPending ? patch.variables?.name : null,
              onToggle: (a, enabled) => patch.mutate({ name: a.name, body: { enabled } }),
              onEdit: (a) => setEditing(a.name),
              onFlag: (a) => setFlagging(a.name),
              onSignIn: (a) => setSignIn(a.name),
              onRemove: (a) => setRemoving(a.name),
            }}
          />
        </div>
      )}

      <AddAccountDialog open={adding} onOpenChange={setAdding} accounts={accounts} onCreated={setSignIn} />
      <ImportDialog open={importing} onOpenChange={setImporting} />
      <SignInDialog name={signIn} onClose={() => setSignIn(null)} />
      <EditAccountSheet account={byName(editing)} onClose={() => setEditing(null)} />
      <FlagDialog account={byName(flagging)} onClose={() => setFlagging(null)} />
      <RemoveAccountDialog name={removing} onClose={() => setRemoving(null)} />
    </>
  )
}
