import { useState } from 'react'
import { useSearchParams } from 'react-router'
import { Plus, SlidersHorizontal } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PageHeader } from '@/components/layout/PageHeader'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { EmptyState } from '@/components/common/EmptyState'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { TableSkeleton } from '@/components/common/TableSkeleton'
import { useLevels, usePutLevels } from '@/api/queries'
import type { Level } from '@/api/types'
import { InstructionEditor } from './InstructionEditor'
import { LevelDialog } from './LevelDialog'
import { LevelsList } from './LevelsList'
import { ModelDialog } from './ModelDialog'
import { ModelsPanel } from './ModelsPanel'
import { RouteLog } from './RouteLog'
import { RoutePreview } from './RoutePreview'

const TABS = ['levels', 'instruction', 'preview', 'log'] as const
type Tab = (typeof TABS)[number]

export function RoutingPage() {
  const [params, setParams] = useSearchParams()
  const raw = params.get('tab')
  const tab: Tab = TABS.includes(raw as Tab) ? (raw as Tab) : 'levels'
  const query = useLevels()
  const put = usePutLevels()
  const [levelTarget, setLevelTarget] = useState<Level | 'new' | null>(null)
  const [modelTarget, setModelTarget] = useState<Level | null>(null)
  const [deleting, setDeleting] = useState<Level | null>(null)
  // Failure of a move or removal made straight in the list (the dialogs show their own).
  const [listError, setListError] = useState<unknown>(null)
  const view = query.data

  const save = (levels: Level[]) => {
    if (!view) return Promise.reject(new Error('Levels are not loaded yet'))
    return put.mutateAsync({ levels, routing_instruction: view.routing_instruction })
  }
  // Dialog saves clear an earlier list error once they succeed.
  const saveFromDialog = (levels: Level[]) => save(levels).then((r) => (setListError(null), r))
  const withModels = (level: Level, models: string[]) => (view?.levels ?? []).map((l) => (l.name === level.name ? { ...l, models } : l))

  const saveFromList = (levels: Level[]) => {
    setListError(null)
    save(levels).catch(setListError)
  }
  const move = (level: Level, index: number, step: -1 | 1) => {
    const models = [...level.models]
    ;[models[index], models[index + step]] = [models[index + step], models[index]]
    saveFromList(withModels(level, models))
  }

  return (
    <>
      <PageHeader
        title="Levels and routing"
        description="Edit levels, preview a Route and review the Route log."
        actions={
          tab === 'levels' ? (
            <Button onClick={() => setLevelTarget('new')} disabled={!view}>
              <Plus aria-hidden /> Add level
            </Button>
          ) : undefined
        }
      />
      <Tabs value={tab} onValueChange={(t) => setParams(t === 'levels' ? {} : { tab: t }, { replace: true })}>
        <TabsList className="mb-4 max-w-full overflow-x-auto">
          <TabsTrigger value="levels">Levels</TabsTrigger>
          <TabsTrigger value="instruction">Routing instruction</TabsTrigger>
          <TabsTrigger value="preview">Preview</TabsTrigger>
          <TabsTrigger value="log">Route log</TabsTrigger>
        </TabsList>

        <TabsContent value="levels" className="flex flex-col gap-4">
          {query.isError && !view ? (
            <ErrorAlert error={query.error} title="Could not load levels" onRetry={() => void query.refetch()} />
          ) : !view ? (
            <TableSkeleton rows={4} columns={3} />
          ) : view.levels.length === 0 ? (
            <EmptyState
              icon={SlidersHorizontal}
              title="No levels yet"
              description="A level maps a range of task complexity to the models the router may pick."
              action={<Button onClick={() => setLevelTarget('new')}>Add level</Button>}
            />
          ) : (
            <LevelsList
              levels={view.levels}
              pending={put.isPending}
              onEdit={setLevelTarget}
              onDelete={setDeleting}
              onAddModel={setModelTarget}
              onMove={move}
              onRemove={(level, ref) =>
                saveFromList(withModels(level, level.models.filter((m) => m !== ref)))
              }
            />
          )}
          {listError !== null && <ErrorAlert error={listError} title="Could not save the levels" />}
          <ModelsPanel />
        </TabsContent>

        <TabsContent value="instruction">
          {query.isError && !view ? (
            <ErrorAlert error={query.error} title="Could not load the instruction" onRetry={() => void query.refetch()} />
          ) : view ? (
            <InstructionEditor view={view} />
          ) : (
            <TableSkeleton rows={3} columns={1} />
          )}
        </TabsContent>

        <TabsContent value="preview">
          <RoutePreview />
        </TabsContent>

        <TabsContent value="log">
          <RouteLog />
        </TabsContent>
      </Tabs>

      <LevelDialog target={levelTarget} levels={view?.levels ?? []} onClose={() => setLevelTarget(null)} onSave={saveFromDialog} />
      <ModelDialog level={modelTarget} onClose={() => setModelTarget(null)} onSave={(level, ref) => saveFromDialog(withModels(level, [...level.models, ref]))} />
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={`Delete level ${deleting?.name ?? ''}?`}
        description="Tasks in its range are routed with the nearest remaining level."
        confirmLabel="Delete level"
        destructive
        pending={put.isPending}
        onConfirm={() => {
          if (!view || !deleting) return
          const name = deleting.name
          setListError(null)
          put
            .mutateAsync({ levels: view.levels.filter((l) => l.name !== name), routing_instruction: view.routing_instruction })
            .then(() => toast.success(`Deleted level ${name}`))
            .catch(setListError)
            .finally(() => setDeleting(null))
        }}
      />
    </>
  )
}
