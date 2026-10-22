import { MoreHorizontal } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { HeadroomBar } from '@/components/common/HeadroomBar'
import { RelativeTime } from '@/components/common/RelativeTime'
import { SignInState } from './SignInState'
import type { AccountView } from '@/api/types'

export interface AccountActions {
  onToggle: (account: AccountView, enabled: boolean) => void
  onEdit: (account: AccountView) => void
  onFlag: (account: AccountView) => void
  onSignIn: (account: AccountView) => void
  onRemove: (account: AccountView) => void
  /** Name of the account whose enable toggle is saving. */
  pending?: string | null
}

export function AccountsTable({ accounts, actions }: { accounts: AccountView[]; actions: AccountActions }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">Name</TableHead>
          <TableHead scope="col">Harness</TableHead>
          <TableHead scope="col">Sign-in</TableHead>
          <TableHead scope="col">Enabled</TableHead>
          <TableHead scope="col" className="text-right">
            Cap
          </TableHead>
          <TableHead scope="col" className="text-right">
            Priority
          </TableHead>
          <TableHead scope="col">Pools</TableHead>
          <TableHead scope="col">Headroom</TableHead>
          <TableHead scope="col">
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {accounts.map((a) => (
          <TableRow key={a.name} data-account={a.name}>
            <TableCell className="font-medium">
              {a.name}
              {a.error && <p className="max-w-48 truncate text-xs font-normal text-destructive" title={a.error}>{a.error}</p>}
            </TableCell>
            <TableCell>
              <HarnessBadge harness={a.harness} />
            </TableCell>
            <TableCell>
              <SignInState state={a.signed_in} />
            </TableCell>
            <TableCell>
              <Switch
                aria-label={`Enable ${a.name}`}
                checked={a.enabled}
                disabled={actions.pending === a.name}
                onCheckedChange={(v) => actions.onToggle(a, v)}
              />
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">
              {a.active_runs}/{a.cap}
            </TableCell>
            <TableCell className="text-right font-mono tabular-nums">{a.priority ?? '-'}</TableCell>
            <TableCell>
              <div className="flex flex-wrap gap-1">
                {(a.pools ?? []).map((p) => (
                  <Badge key={p} variant="outline">
                    {p}
                  </Badge>
                ))}
                {(a.flags ?? []).map((f) => (
                  <Badge key={`${f.pool ?? 'all'}-${f.until}`} variant="destructive" title={f.reason}>
                    cooldown {f.pool ?? 'all'} <RelativeTime value={f.until} />
                  </Badge>
                ))}
              </div>
            </TableCell>
            <TableCell>
              {a.headroom.length === 0 ? (
                <HeadroomBar pct={null} label="5h" />
              ) : (
                <div className="flex flex-col gap-1">
                  {a.headroom.map((h) => (
                    <div key={h.pool} className="flex items-center gap-2">
                      <span className="w-14 truncate text-xs text-muted-foreground" title={h.pool}>
                        {h.pool}
                      </span>
                      <HeadroomBar pct={h.five_hour_remaining_pct} status={h.status} resetsAt={h.five_hour_resets_at} />
                    </div>
                  ))}
                </div>
              )}
            </TableCell>
            <TableCell>
              <div className="flex items-center justify-end gap-1">
                {a.signed_in !== 'signed_in' && (
                  <Button size="sm" variant="outline" onClick={() => actions.onSignIn(a)}>
                    Sign in<span className="sr-only"> {a.name}</span>
                  </Button>
                )}
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button size="icon-sm" variant="ghost" aria-label={`Actions for ${a.name}`}>
                      <MoreHorizontal aria-hidden />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onSelect={() => actions.onEdit(a)}>Edit</DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => actions.onFlag(a)}>Flag cooldown</DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => actions.onSignIn(a)}>Sign in</DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem variant="destructive" onSelect={() => actions.onRemove(a)}>
                      Remove
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
