import { ArrowDown, ArrowUp, Pencil, Plus, Trash2, X } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import type { Level } from '@/api/types'
import { splitRef } from './schemas'

interface Props {
  levels: Level[]
  pending?: boolean
  onEdit: (level: Level) => void
  onDelete: (level: Level) => void
  onAddModel: (level: Level) => void
  /** Moves the model at `index` one place up (`-1`) or down (`1`). */
  onMove: (level: Level, index: number, step: -1 | 1) => void
  onRemove: (level: Level, ref: string) => void
}

/** One card per Level in config order; the models of a level are listed in preference order. */
export function LevelsList({ levels, pending, onEdit, onDelete, onAddModel, onMove, onRemove }: Props) {
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      {levels.map((level) => (
        <Card key={level.name} role="region" aria-label={`Level ${level.name}`} data-level={level.name} className="min-w-0">
          <CardHeader>
            <CardTitle className="flex flex-wrap items-center gap-2 text-base">
              <span>{level.name}</span>
              <span className="text-sm font-normal tabular-nums text-muted-foreground">
                {level.min}–{level.max}
              </span>
              {level.tier && <Badge variant="outline">tier {level.tier}</Badge>}
            </CardTitle>
            <CardAction className="flex flex-wrap justify-end gap-1">
              <Button variant="outline" size="sm" disabled={pending} onClick={() => onEdit(level)}>
                <Pencil aria-hidden /> Edit level
              </Button>
              <Button
                variant="ghost"
                size="sm"
                disabled={pending || levels.length === 1}
                title={levels.length === 1 ? 'The last level cannot be deleted' : undefined}
                aria-description={levels.length === 1 ? 'The last level cannot be deleted' : undefined}
                onClick={() => onDelete(level)}
              >
                <Trash2 aria-hidden /> Delete level
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            {level.models.length === 0 ? (
              <p className="text-sm text-muted-foreground">No models listed: this level uses any eligible model of its tier.</p>
            ) : (
              <ol aria-label={`Models of ${level.name}`} className="flex flex-col gap-2">
                {level.models.map((ref, i) => (
                  <li key={ref} className="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
                    <HarnessBadge harness={splitRef(ref).harness} />
                    <span className="min-w-0 flex-1 font-mono text-xs break-all">{ref}</span>
                    <span className="flex gap-1">
                      <Button size="icon-sm" variant="ghost" aria-label={`Move ${ref} up`} disabled={pending || i === 0} onClick={() => onMove(level, i, -1)}>
                        <ArrowUp aria-hidden />
                      </Button>
                      <Button size="icon-sm" variant="ghost" aria-label={`Move ${ref} down`} disabled={pending || i === level.models.length - 1} onClick={() => onMove(level, i, 1)}>
                        <ArrowDown aria-hidden />
                      </Button>
                      <Button size="icon-sm" variant="ghost" aria-label={`Remove ${ref}`} disabled={pending} onClick={() => onRemove(level, ref)}>
                        <X aria-hidden />
                      </Button>
                    </span>
                  </li>
                ))}
              </ol>
            )}
            <Button variant="outline" size="sm" className="self-start" disabled={pending} onClick={() => onAddModel(level)}>
              <Plus aria-hidden /> Add model
            </Button>
          </CardContent>
        </Card>
      ))}
    </div>
  )
}
