import { useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ArrowDown, ArrowUp, Layers, Pencil, Plus, RotateCcw, RotateCw, Trash2, X } from 'lucide-react'
import { toast } from 'sonner'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { ScrollTable } from '@/components/common/ScrollTable'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { qk, useChoices, useLevels, usePutChoices, usePutLevels, useResetLevels } from '@/api/queries'
import { errorMessage } from '@/lib/errors'
import type { Choice, Level, LevelChoiceView, LevelView, LevelsView } from '@/api/types'
import { AddModelDialog } from './AddModelDialog'
import { ChoiceDialog } from './ChoiceDialog'
import { LevelDialog } from './LevelDialog'
import { levelOf } from './schemas'

function State({ c }: { c: LevelChoiceView }) {
  if (!c.known) return <Badge variant="destructive">unknown</Badge>
  return <Badge variant={c.enabled ? 'secondary' : 'outline'}>{c.enabled ? 'on' : 'off'}</Badge>
}

interface TableProps {
  level: LevelView
  pending: boolean
  onMove: (level: LevelView, id: string, by: -1 | 1) => void
  onRemove: (level: LevelView, id: string) => void
}

function ModelsTable({ level, pending, onMove, onRemove }: TableProps) {
  const last = level.choices.length - 1
  return (
    <ScrollTable>
      <Table aria-label={`Models in ${level.name}`}>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">Priority</TableHead>
            <TableHead scope="col">Choice</TableHead>
            <TableHead scope="col">Harness</TableHead>
            <TableHead scope="col">Model</TableHead>
            <TableHead scope="col">Effort</TableHead>
            <TableHead scope="col">State</TableHead>
            <TableHead scope="col">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {level.choices.map((c, i) => (
            <TableRow key={c.id} data-level-choice={c.id}>
              <TableCell>{c.priority ?? '-'}</TableCell>
              <TableCell className="font-medium">{c.id}</TableCell>
              <TableCell>{c.harness ? <HarnessBadge harness={c.harness} /> : '-'}</TableCell>
              <TableCell className="font-mono text-xs">{c.model ?? '-'}</TableCell>
              <TableCell>{c.effort ?? '-'}</TableCell>
              <TableCell>
                <State c={c} />
              </TableCell>
              <TableCell className="text-right whitespace-nowrap">
                <Button size="icon-sm" variant="ghost" aria-label={`Move ${c.id} up in ${level.name}`} disabled={pending || i === 0} onClick={() => onMove(level, c.id, -1)}>
                  <ArrowUp aria-hidden />
                </Button>
                <Button size="icon-sm" variant="ghost" aria-label={`Move ${c.id} down in ${level.name}`} disabled={pending || i === last} onClick={() => onMove(level, c.id, 1)}>
                  <ArrowDown aria-hidden />
                </Button>
                <Button size="icon-sm" variant="ghost" aria-label={`Remove ${c.id} from ${level.name}`} disabled={pending} onClick={() => onRemove(level, c.id)}>
                  <X aria-hidden />
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </ScrollTable>
  )
}

/** The Levels tab: complexity levels and the models each one allows. */
export function LevelsPanel() {
  const qc = useQueryClient()
  const query = useLevels()
  const choicesQuery = useChoices()
  const putLevels = usePutLevels()
  const putChoices = usePutChoices()
  const reset = useResetLevels()
  const [levelTarget, setLevelTarget] = useState<LevelView | 'new' | null>(null)
  const [deleting, setDeleting] = useState<LevelView | null>(null)
  const [adding, setAdding] = useState<LevelView | null>(null)
  const [newModelFor, setNewModelFor] = useState<LevelView | null>(null)
  const [resetting, setResetting] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const queue = useRef<Promise<unknown>>(Promise.resolve())
  const view = query.data
  const choices = choicesQuery.data?.choices ?? []
  const pending = putLevels.isPending || putChoices.isPending || reset.isPending

  /**
   * Runs saves and resets one at a time. `save` builds its list from the cache after the previous job
   * refetched it, so quick edits never overwrite each other. Any new action clears the "Added" notice.
   */
  const enqueue = <T,>(job: () => Promise<T>) => {
    setNotice(null)
    const next = queue.current.catch(() => undefined).then(job)
    queue.current = next
    return next
  }
  const save = (update: (current: LevelView[]) => Level[]) =>
    enqueue(() => putLevels.mutateAsync({ levels: update(qc.getQueryData<LevelsView>(qk.levels())?.levels ?? []) }))
  /** Row actions have no dialog, so a failure shows above the list. */
  const run = (update: (current: LevelView[]) => Level[], done?: string) => {
    setActionError(null)
    save(update).then(
      () => done && toast.success(done),
      (e) => setActionError(errorMessage(e)),
    )
  }
  const edit = (name: string, change: (l: Level) => Level['choices']) => (current: LevelView[]) => {
    if (!current.some((l) => l.name === name)) throw new Error(`Level ${name} no longer exists`)
    return current.map((l) => (l.name === name ? { ...levelOf(l), choices: change(levelOf(l)) } : levelOf(l)))
  }

  const move = (level: LevelView, id: string, by: -1 | 1) =>
    run(
      edit(level.name, (l) => {
        const ids = (l.choices ?? []).map((c) => c.id)
        const i = ids.indexOf(id)
        if (i < 0 || i + by < 0 || i + by >= ids.length) return l.choices
        ;[ids[i], ids[i + by]] = [ids[i + by], ids[i]]
        return ids.map((x, k) => ({ id: x, priority: k + 1 }))
      }),
    )
  const remove = (level: LevelView, id: string) => run(edit(level.name, (l) => l.choices?.filter((c) => c.id !== id)), `Removed ${id} from ${level.name}`)
  const addModel = (level: LevelView, id: string, priority: number | undefined) =>
    save(edit(level.name, (l) => [...(l.choices ?? []), { id, ...(priority !== undefined ? { priority } : {}) }]))
  /** Saves the new choice first, then lists it in the level; the second step never reads refetched choices. */
  const saveNewChoice = async (list: Choice[]) => {
    const level = newModelFor
    setNotice(null)
    const created = list.find((c) => !choices.some((x) => x.id === c.id))
    await putChoices.mutateAsync({ choices: list, routing_instruction: choicesQuery.data?.routing_instruction ?? '' })
    if (!level || !created) return
    setActionError(null)
    try {
      await addModel(level, created.id, undefined)
      setNotice(`Added ${created.id} to ${level.name}`)
    } catch (e) {
      setActionError(`Choice ${created.id} created, but adding it to ${level.name} failed: ${errorMessage(e)}`)
    }
  }
  const all = view?.levels ?? []

  if (query.isError && !view) return <ErrorAlert error={query.error} title="Could not load levels" onRetry={() => void query.refetch()} />
  if (!view) return <TableSkeleton rows={4} columns={7} />

  return (
    <div className="flex flex-col gap-4">
      {view.restart_required && (
        <Alert>
          <RotateCw aria-hidden />
          <AlertTitle>Restart required</AlertTitle>
          <AlertDescription>The saved levels differ from what the running supervisor loaded. Restart the supervisor to apply them.</AlertDescription>
        </Alert>
      )}
      {view.default_levels && (
        <Alert>
          <Layers aria-hidden />
          <AlertTitle>Using the default levels</AlertTitle>
          <AlertDescription>
            <p>No levels are saved in config.toml, so each range falls back to a model class (tier). Editing any level saves the whole list.</p>
            <ul className="mt-1 list-disc pl-5">
              {view.levels.map((l) => (
                <li key={l.name}>
                  {l.name} {l.min}–{l.max}: {l.tier}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => setLevelTarget('new')}>
          <Plus aria-hidden /> Add level
        </Button>
        <Button variant="outline" onClick={() => setResetting(true)} disabled={pending}>
          <RotateCcw aria-hidden /> Reset to defaults
        </Button>
      </div>
      {notice && (
        <p role="status" className="text-sm">
          {notice}
        </p>
      )}
      {actionError && <ErrorAlert error={actionError} title="Could not save levels" />}

      {view.levels.length === 0 ? (
        <EmptyState icon={Layers} title="No levels" description="Levels are disabled: the router picks among all eligible choices." action={<Button onClick={() => setLevelTarget('new')}>Add level</Button>} />
      ) : (
        view.levels.map((l, n) => (
          <section key={l.name} aria-labelledby={`level-${n}`} className="flex flex-col gap-2 rounded-lg border p-4" data-level={l.name}>
            <div className="flex flex-wrap items-center gap-2">
              <h2 id={`level-${n}`} className="text-base font-semibold">
                {l.name}
              </h2>
              <Badge variant="outline">
                {l.min}–{l.max}
              </Badge>
              {l.tier && <Badge variant="secondary">tier {l.tier}</Badge>}
              <div className="ml-auto flex flex-wrap gap-1">
                <Button size="sm" variant="outline" aria-label={`Add model to ${l.name}`} onClick={() => setAdding(l)} disabled={pending}>
                  <Plus aria-hidden /> Add model
                </Button>
                <Button size="sm" variant="outline" aria-label={`New model for ${l.name}`} onClick={() => setNewModelFor(l)} disabled={pending}>
                  <Plus aria-hidden /> New model
                </Button>
                <Button size="sm" variant="ghost" aria-label={`Edit ${l.name}`} onClick={() => setLevelTarget(l)} disabled={pending}>
                  <Pencil aria-hidden /> Edit
                </Button>
                <Button size="sm" variant="ghost" aria-label={`Delete ${l.name}`} onClick={() => setDeleting(l)} disabled={pending}>
                  <Trash2 aria-hidden /> Delete
                </Button>
              </div>
            </div>
            {l.choices.length === 0 ? (
              <p className="text-sm text-muted-foreground">{l.tier ? `No models listed: any eligible ${l.tier} model is used.` : 'No models listed.'}</p>
            ) : (
              <ModelsTable level={l} pending={pending} onMove={move} onRemove={remove} />
            )}
          </section>
        ))
      )}

      <LevelDialog target={levelTarget} levels={all} pending={pending} onClose={() => setLevelTarget(null)} onSave={save} />
      <AddModelDialog level={adding} choices={choices} pending={pending} onClose={() => setAdding(null)} onSave={addModel} />
      <ChoiceDialog target={newModelFor ? 'new' : null} choices={choices} pending={pending} description={newModelFor ? `The new choice is added to the ${newModelFor.name} level.` : undefined} onClose={() => setNewModelFor(null)} onSave={saveNewChoice} />
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={`Delete ${deleting?.name ?? 'level'}?`}
        description="Complexity in this range will match no level."
        confirmLabel="Delete level"
        destructive
        pending={pending}
        onConfirm={() => {
          if (!deleting) return
          const name = deleting.name
          setActionError(null)
          save((cur) => cur.filter((l) => l.name !== name).map(levelOf)).then(
            () => {
              toast.success(`Deleted ${name}`)
              setDeleting(null)
            },
            (e) => {
              setActionError(errorMessage(e))
              setDeleting(null)
            },
          )
        }}
      />
      <ConfirmDialog
        open={resetting}
        onOpenChange={setResetting}
        title="Reset levels to defaults?"
        description="This removes the saved levels from config.toml. The default tier-based levels apply again."
        confirmLabel="Reset levels"
        destructive
        pending={pending}
        onConfirm={() => {
          setActionError(null)
          enqueue(() => reset.mutateAsync()).then(
            () => {
              toast.success('Levels reset to defaults')
              setResetting(false)
            },
            (e) => {
              setActionError(errorMessage(e))
              setResetting(false)
            },
          )
        }}
      />
    </div>
  )
}
