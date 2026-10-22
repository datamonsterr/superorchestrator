import { Link } from 'react-router'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { SignInState } from '@/components/common/SignInState'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { HeadroomBar } from '@/components/common/HeadroomBar'
import { EmptyState } from '@/components/common/EmptyState'
import { Users } from 'lucide-react'
import type { AccountView } from '@/api/types'
import { ScrollTable } from '@/components/common/ScrollTable'

/** One row per (account, pool); accounts without headroom data get a single unknown row. */
export function AccountsSummaryTable({ accounts }: { accounts: AccountView[] }) {
  if (accounts.length === 0) {
    return (
      <EmptyState
        icon={Users}
        title="No accounts yet"
        description="Add your first account to start routing work."
        action={
          <Link className="text-sm underline underline-offset-4" to="/accounts">
            Add an account
          </Link>
        }
      />
    )
  }
  return (
    <ScrollTable className="border-0">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">Account</TableHead>
          <TableHead scope="col">Harness</TableHead>
          <TableHead scope="col">Sign-in</TableHead>
          <TableHead scope="col">Pool</TableHead>
          <TableHead scope="col">5h headroom</TableHead>
          <TableHead scope="col">Weekly</TableHead>
          <TableHead scope="col" className="text-right">
            Runs
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {accounts.map((a) => {
          const rows = a.headroom.length ? a.headroom : [null]
          return rows.map((h, i) => (
            <TableRow key={`${a.name}-${h?.pool ?? 'none'}`} data-account={a.name} className={!a.enabled ? 'opacity-60' : undefined}>
              {i === 0 ? (
                <>
                  <TableCell rowSpan={rows.length} className="align-top font-medium">
                    <Link to="/accounts" className="underline-offset-4 hover:underline">
                      {a.name}
                    </Link>
                    {!a.enabled && <span className="ml-2 text-xs text-muted-foreground">disabled</span>}
                  </TableCell>
                  <TableCell rowSpan={rows.length} className="align-top">
                    <HarnessBadge harness={a.harness} />
                  </TableCell>
                  <TableCell rowSpan={rows.length} className="align-top">
                    <SignInState state={a.signed_in} />
                  </TableCell>
                </>
              ) : null}
              <TableCell className="text-muted-foreground">{h?.pool ?? '-'}</TableCell>
              <TableCell>
                <HeadroomBar pct={h?.five_hour_remaining_pct} status={h?.status} resetsAt={h?.five_hour_resets_at} />
              </TableCell>
              <TableCell>
                <HeadroomBar pct={h?.weekly_remaining_pct} status={h?.status === 'exhausted' ? undefined : h?.status} resetsAt={h?.weekly_resets_at} />
              </TableCell>
              {i === 0 ? (
                <TableCell rowSpan={rows.length} className="text-right align-top font-mono tabular-nums">
                  {a.active_runs}/{a.cap}
                </TableCell>
              ) : null}
            </TableRow>
          ))
        })}
      </TableBody>
    </Table>
    </ScrollTable>
  )
}
