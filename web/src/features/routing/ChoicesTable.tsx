import { MoreHorizontal } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import type { Choice } from '@/api/types'

interface Props {
  choices: Choice[]
  pending?: boolean
  onToggle: (choice: Choice, enabled: boolean) => void
  onEdit: (choice: Choice) => void
  onDelete: (choice: Choice) => void
}

export function ChoicesTable({ choices, pending, onToggle, onEdit, onDelete }: Props) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">Id</TableHead>
          <TableHead scope="col">Harness</TableHead>
          <TableHead scope="col">Model</TableHead>
          <TableHead scope="col">Effort</TableHead>
          <TableHead scope="col">Roles</TableHead>
          <TableHead scope="col">Enabled</TableHead>
          <TableHead scope="col">Description</TableHead>
          <TableHead scope="col">
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {choices.map((c) => (
          <TableRow key={c.id} data-choice={c.id} className={c.enabled ? undefined : 'opacity-60'}>
            <TableCell className="font-medium">{c.id}</TableCell>
            <TableCell>
              <HarnessBadge harness={c.harness} />
            </TableCell>
            <TableCell className="font-mono text-xs">{c.model}</TableCell>
            <TableCell>{c.effort ?? '-'}</TableCell>
            <TableCell>
              <div className="flex flex-wrap gap-1">
                {c.roles.map((r) => (
                  <Badge key={r} variant="outline">
                    {r}
                  </Badge>
                ))}
              </div>
            </TableCell>
            <TableCell>
              <Switch aria-label={`Enable ${c.id}`} checked={c.enabled} disabled={pending} onCheckedChange={(v) => onToggle(c, v)} />
            </TableCell>
            <TableCell className="max-w-64 truncate text-muted-foreground" title={c.description}>
              {c.description}
            </TableCell>
            <TableCell className="text-right">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon-sm" variant="ghost" aria-label={`Actions for ${c.id}`}>
                    <MoreHorizontal aria-hidden />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onSelect={() => onEdit(c)}>Edit</DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" onSelect={() => onDelete(c)}>
                    Delete
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
