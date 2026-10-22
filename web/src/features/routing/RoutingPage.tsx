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
import { ScrollTable } from '@/components/common/ScrollTable'
import { useChoices, usePutChoices } from '@/api/queries'
import type { Choice } from '@/api/types'
import { ChoiceDialog } from './ChoiceDialog'
import { ChoicesTable } from './ChoicesTable'
import { InstructionEditor } from './InstructionEditor'
import { ModelsPanel } from './ModelsPanel'
import { RouteLog } from './RouteLog'
import { RoutePreview } from './RoutePreview'

const TABS = ['choices', 'instruction', 'preview', 'log'] as const
type Tab = (typeof TABS)[number]

export function RoutingPage() {
  const [params, setParams] = useSearchParams()
  const raw = params.get('tab')
  const tab: Tab = TABS.includes(raw as Tab) ? (raw as Tab) : 'choices'
  const query = useChoices()
  const put = usePutChoices()
  const [target, setTarget] = useState<Choice | 'new' | null>(null)
  const [deleting, setDeleting] = useState<Choice | null>(null)
  const view = query.data

  const save = (choices: Choice[]) => {
    if (!view) return Promise.reject(new Error('Choices are not loaded yet'))
    return put.mutateAsync({ choices, routing_instruction: view.routing_instruction })
  }

  return (
    <>
      <PageHeader
        title="Choices and routing"
        description="Edit Choices, preview a Route and review the Route log."
        actions={
          tab === 'choices' ? (
            <Button onClick={() => setTarget('new')} disabled={!view}>
              <Plus aria-hidden /> Add choice
            </Button>
          ) : undefined
        }
      />
      <Tabs value={tab} onValueChange={(t) => setParams(t === 'choices' ? {} : { tab: t }, { replace: true })}>
        <TabsList className="mb-4 max-w-full overflow-x-auto">
          <TabsTrigger value="choices">Choices</TabsTrigger>
          <TabsTrigger value="instruction">Routing instruction</TabsTrigger>
          <TabsTrigger value="preview">Preview</TabsTrigger>
          <TabsTrigger value="log">Route log</TabsTrigger>
        </TabsList>

        <TabsContent value="choices" className="flex flex-col gap-4">
          {query.isError && !view ? (
            <ErrorAlert error={query.error} title="Could not load choices" onRetry={() => void query.refetch()} />
          ) : !view ? (
            <TableSkeleton rows={4} columns={7} />
          ) : view.choices.length === 0 ? (
            <EmptyState
              icon={SlidersHorizontal}
              title="No choices yet"
              description="Choices are the harness and model combinations the router may pick."
              action={<Button onClick={() => setTarget('new')}>Add choice</Button>}
            />
          ) : (
            <ScrollTable>
              <ChoicesTable
                choices={view.choices}
                pending={put.isPending}
                onToggle={(c, enabled) => put.mutate({ choices: view.choices.map((x) => (x.id === c.id ? { ...x, enabled } : x)), routing_instruction: view.routing_instruction })}
                onEdit={setTarget}
                onDelete={setDeleting}
              />
            </ScrollTable>
          )}
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

      <ChoiceDialog target={target} choices={view?.choices ?? []} onClose={() => setTarget(null)} onSave={save} />
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={`Delete ${deleting?.id ?? 'choice'}?`}
        description="The router will no longer pick this choice."
        confirmLabel="Delete choice"
        destructive
        pending={put.isPending}
        onConfirm={() => {
          if (!view || !deleting) return
          const id = deleting.id
          put.mutate(
            { choices: view.choices.filter((c) => c.id !== id), routing_instruction: view.routing_instruction },
            {
              onSuccess: () => {
                toast.success(`Deleted ${id}`)
                setDeleting(null)
              },
            },
          )
        }}
      />
    </>
  )
}
