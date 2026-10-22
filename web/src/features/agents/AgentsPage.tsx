import { Link } from 'react-router'
import { Bot, Plus } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { PageHeader } from '@/components/layout/PageHeader'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { NotAvailable } from '@/components/common/NotAvailable'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { ScrollTable } from '@/components/common/ScrollTable'
import { isNotAvailable } from '@/lib/errors'
import { useAgents } from '@/api/queries'

const SOURCE_LABEL: Record<string, string> = { user: 'User', superorchestrator: 'Config', claude: 'Claude (read-only)' }

export function AgentsPage() {
  const query = useAgents()
  const agents = query.data?.agents ?? []
  return (
    <>
      <PageHeader
        title="Agents"
        description="Custom agents defined by AGENT.md files."
        actions={
          <Button asChild>
            <Link to="/agents/new">
              <Plus aria-hidden /> New agent
            </Link>
          </Button>
        }
      />
      {query.isError && !query.data ? (
        isNotAvailable(query.error) ? (
          <NotAvailable what="Custom agents" />
        ) : (
          <ErrorAlert error={query.error} title="Could not load agents" onRetry={() => void query.refetch()} />
        )
      ) : query.isPending ? (
        <TableSkeleton rows={3} columns={5} />
      ) : agents.length === 0 ? (
        <EmptyState
          icon={Bot}
          title="No custom agents yet"
          description="An agent is a prompt, a preferred harness and some limits, saved as an AGENT.md file."
          action={
            <Button asChild>
              <Link to="/agents/new">New agent</Link>
            </Button>
          }
        />
      ) : (
        <ScrollTable>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Name</TableHead>
                <TableHead scope="col">Description</TableHead>
                <TableHead scope="col">Source</TableHead>
                <TableHead scope="col">Harness</TableHead>
                <TableHead scope="col">Path</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {agents.map((a) => (
                <TableRow key={a.name}>
                  <TableCell className="font-medium">
                    <Link to={`/agents/${encodeURIComponent(a.name)}`} className="underline-offset-4 hover:underline">
                      {a.name}
                    </Link>
                  </TableCell>
                  <TableCell className="max-w-xs truncate">{a.description}</TableCell>
                  <TableCell>
                    <Badge variant={a.source === 'claude' ? 'outline' : 'secondary'}>{SOURCE_LABEL[a.source ?? ''] ?? a.source ?? 'unknown'}</Badge>
                  </TableCell>
                  <TableCell>{a.harness ?? 'auto'}</TableCell>
                  <TableCell className="max-w-xs truncate font-mono text-xs text-muted-foreground" title={a.path ?? undefined}>
                    {a.path}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </ScrollTable>
      )}
    </>
  )
}
