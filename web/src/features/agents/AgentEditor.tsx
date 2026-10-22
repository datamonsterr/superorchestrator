import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import { toast } from 'sonner'
import { AlertTriangle, ArrowLeft, Lock, Trash2 } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { PageHeader } from '@/components/layout/PageHeader'
import { ConfirmDialog } from '@/components/common/ConfirmDialog'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { NotAvailable } from '@/components/common/NotAvailable'
import { isEndpointMissing } from '@/lib/errors'
import { ApiError } from '@/api/client'
import { useAgent, useCreateAgent, useDeleteAgent, usePutAgent } from '@/api/queries'
import type { AgentDetail } from '@/api/types'
import { NEW_AGENT_TEMPLATE, readFrontmatter } from './frontmatter'

/** The save error from a 422: the message plus any `data` entries (field: message). */
function SaveError({ error }: { error: unknown }) {
  const data = error instanceof ApiError && error.data && typeof error.data === 'object' && !Array.isArray(error.data) ? Object.entries(error.data) : []
  return (
    <Alert variant="destructive" role="alert">
      <AlertTriangle aria-hidden />
      <AlertTitle>The agent file was not saved</AlertTitle>
      <AlertDescription>
        <p>{error instanceof Error ? error.message : 'Unknown error'}</p>
        {data.length > 0 && (
          <ul className="list-disc pl-4">
            {data.map(([k, v]) => (
              <li key={k}>
                <span className="font-mono">{k}</span>: {Array.isArray(v) ? v.join(', ') : String(v)}
              </li>
            ))}
          </ul>
        )}
      </AlertDescription>
    </Alert>
  )
}

function FrontmatterCard({ source }: { source: string }) {
  const { fields, hints } = readFrontmatter(source)
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Frontmatter</CardTitle>
      </CardHeader>
      <CardContent className="space-y-2 text-sm">
        {fields && Object.keys(fields).length > 0 && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            {Object.entries(fields).map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="font-mono text-muted-foreground">{k}</dt>
                <dd className="truncate">{v}</dd>
              </div>
            ))}
          </dl>
        )}
        {hints.length > 0 && (
          <ul className="list-disc pl-4 text-muted-foreground" aria-label="Frontmatter hints">
            {hints.map((h) => (
              <li key={h}>{h}</li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  )
}

interface FormProps {
  /** Existing agent, or undefined when creating. */
  agent?: AgentDetail
}

function EditorForm({ agent }: FormProps) {
  const navigate = useNavigate()
  const create = useCreateAgent()
  const put = usePutAgent()
  const del = useDeleteAgent()
  const [name, setName] = useState('')
  const [source, setSource] = useState(agent?.source ?? NEW_AGENT_TEMPLATE)
  // The file as this draft started from it; a refetch that differs means it changed on disk.
  const [base, setBase] = useState(agent?.source)
  const [warnings, setWarnings] = useState<string[]>([])
  const [confirmDelete, setConfirmDelete] = useState(false)

  const hasMarker = source.includes('[REDACTED]')
  const readOnly = agent?.read_only ?? false
  const redacted = agent?.redacted ?? false
  const saving = create.isPending || put.isPending
  const dirty = agent ? source !== agent.source : true
  const canSave = !readOnly && !redacted && !hasMarker && !saving && dirty && (agent !== undefined || name.trim() !== '')
  const saveError = create.error ?? put.error
  const diskChanged = agent !== undefined && agent.source !== base
  if (diskChanged && (source === base || source === agent.source)) {
    // Nothing of ours to lose: follow the file.
    setBase(agent.source)
    setSource(agent.source)
  }
  const staleDraft = diskChanged && source !== base && source !== agent.source

  async function save() {
    setWarnings([])
    try {
      const res = agent ? await put.mutateAsync({ name: agent.name, source }) : await create.mutateAsync({ name: name.trim(), source })
      setWarnings(res.warnings)
      setBase(source)
      toast.success(agent ? 'Agent saved' : 'Agent created')
      if (!agent) void navigate(`/agents/${encodeURIComponent(name.trim())}`, { replace: true })
    } catch {
      /* shown inline from the mutation state */
    }
  }

  return (
    <div className="space-y-4">
      {readOnly && (
        <Alert>
          <Lock aria-hidden />
          <AlertTitle>Read-only import</AlertTitle>
          <AlertDescription>
            This agent is imported from <span className="font-mono">.claude/agents</span> and cannot be edited or removed here. Copy it to the config agents
            folder to change it.
          </AlertDescription>
        </Alert>
      )}
      {redacted && (
        <Alert variant="destructive" role="alert">
          <AlertTriangle aria-hidden />
          <AlertTitle>Secret values are hidden</AlertTitle>
          <AlertDescription>
            Values that look like secrets were replaced with <span className="font-mono">[REDACTED]</span>. Saving is disabled so the marker is not written
            over the real value. Edit the file directly: <span className="font-mono break-all">{agent?.path}</span>
          </AlertDescription>
        </Alert>
      )}
      {!agent && (
        <label className="flex max-w-sm flex-col gap-1.5 text-sm">
          <span className="font-medium">Name</span>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="my-agent" autoComplete="off" spellCheck={false} />
          <span className="text-xs text-muted-foreground">Must match the frontmatter name. Letters, digits, dashes.</span>
        </label>
      )}
      <div className="grid gap-4 lg:grid-cols-[1fr_16rem]">
        <label className="flex flex-col gap-1.5 text-sm">
          <span className="font-medium">AGENT.md</span>
          <Textarea
            value={source}
            onChange={(e) => setSource(e.target.value)}
            readOnly={readOnly || redacted}
            spellCheck={false}
            className="min-h-96 font-mono text-xs"
          />
        </label>
        <FrontmatterCard source={source} />
      </div>
      {hasMarker && !redacted && (
        <p role="alert" className="text-sm text-destructive">
          Remove the <span className="font-mono">[REDACTED]</span> markers before saving; they would overwrite real values.
        </p>
      )}
      {staleDraft && agent && (
        <Alert role="alert">
          <AlertTriangle aria-hidden />
          <AlertTitle>Changed on disk</AlertTitle>
          <AlertDescription>
            <p>The file was changed outside this page while you were editing. Saving overwrites it.</p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="mt-2"
              onClick={() => {
                setSource(agent.source)
                setBase(agent.source)
              }}
            >
              Reload from disk (discard my edits)
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {saveError && <SaveError error={saveError} />}
      {warnings.length > 0 && (
        <Alert>
          <AlertTriangle aria-hidden />
          <AlertTitle>Saved with warnings</AlertTitle>
          <AlertDescription>
            <ul className="list-disc pl-4">
              {warnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      )}
      <div data-slot="form-actions" className="flex flex-wrap items-center gap-2 max-md:[&_button]:min-h-10">
        <Button onClick={() => void save()} disabled={!canSave}>
          {saving ? 'Saving...' : agent ? 'Save' : 'Create agent'}
        </Button>
        {agent && (
          <Button variant="outline" onClick={() => setConfirmDelete(true)} disabled={readOnly}>
            <Trash2 aria-hidden /> Delete
          </Button>
        )}
      </div>
      {agent && (
        <ConfirmDialog
          open={confirmDelete}
          onOpenChange={setConfirmDelete}
          title={`Delete ${agent.name}?`}
          description="The agent's folder is removed from disk. This cannot be undone."
          confirmLabel="Delete"
          destructive
          pending={del.isPending}
          onConfirm={() =>
            del.mutate(
              { name: agent.name },
              {
                onSuccess: () => {
                  toast.success(`Deleted ${agent.name}`)
                  void navigate('/agents')
                },
                onSettled: () => setConfirmDelete(false),
              },
            )
          }
        />
      )}
    </div>
  )
}

export function AgentEditor() {
  const { name = '' } = useParams()
  const creating = name === 'new'
  const query = useAgent(creating ? '' : name)
  const agent = query.data
  return (
    <>
      <PageHeader
        title={creating ? 'New agent' : name}
        description={
          agent ? (
            <span className="inline-flex flex-wrap items-center gap-2">
              {agent.description}
              <Badge variant="secondary">{agent.origin}</Badge>
            </span>
          ) : (
            'Edit an AGENT.md file.'
          )
        }
        actions={
          <Button variant="ghost" asChild>
            <Link to="/agents">
              <ArrowLeft aria-hidden /> All agents
            </Link>
          </Button>
        }
      />
      {creating ? (
        <EditorForm />
      ) : query.isError ? (
        isEndpointMissing(query.error) ? (
          <NotAvailable what="Custom agents" />
        ) : (
        <ErrorAlert error={query.error} title="Could not load the agent" onRetry={() => void query.refetch()} />
        )
      ) : !agent ? (
        <div role="status" aria-busy="true" aria-label="Loading">
          <Skeleton className="h-96 w-full" />
        </div>
      ) : (
        <EditorForm key={agent.name} agent={agent} />
      )}
    </>
  )
}
