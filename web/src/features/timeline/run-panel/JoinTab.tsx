import { useEffect, useRef } from 'react'
import { useJoinRun } from '@/api/queries'
import { CommandBlock } from '@/components/common/CommandBlock'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { joinCommand } from './data'

/**
 * Join command for attaching a terminal to the run. `POST /runs/:id/join` is called when this tab opens (the server
 * records a `join_command_issued` audit event). Only env variable names are shown, never values (design 7.5).
 */
export function JoinTab({ runId }: { runId: string }) {
  const join = useJoinRun()
  const { mutate } = join
  const asked = useRef(false)
  useEffect(() => {
    if (asked.current) return
    asked.current = true
    mutate({ id: runId })
  }, [mutate, runId])

  if (join.isError) return <ErrorAlert error={join.error} title="Could not get the join command" onRetry={() => join.mutate({ id: runId })} />
  if (!join.data) return <Skeleton className="h-20" />
  const { argv, cwd, env_names } = join.data
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">Run this in a terminal to attach to the run. Secret values are redacted by the server.</p>
      <CommandBlock command={joinCommand(argv)} cwd={cwd} />
      {env_names.length > 0 && (
        <p className="text-sm">
          sets: <code className="font-mono text-xs">{env_names.join(', ')}</code>
        </p>
      )}
      <div>
        <Button type="button" variant="outline" size="sm" title="Records a join_command_issued audit event each time" disabled={join.isPending} onClick={() => join.mutate({ id: runId })}>
          Issue again
        </Button>
      </div>
    </div>
  )
}
