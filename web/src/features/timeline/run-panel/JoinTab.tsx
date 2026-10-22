import { useEffect, useRef } from 'react'
import type { UseMutationResult } from '@tanstack/react-query'
import type { JoinView } from '@/api/generated'
import { CommandBlock } from '@/components/common/CommandBlock'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { joinCommand, shellQuote } from './data'

type JoinMutation = UseMutationResult<JoinView, Error, { id: string }>

/**
 * Join command for attaching a terminal to the run. The mutation lives in the panel, so the POST (which records a
 * `join_command_issued` audit event) happens once when the tab is first opened and its result survives tab
 * switches; "Issue again" is explicit. Only env variable names are shown, never values (design 7.5). The cwd is
 * shell-quoted: the command is meant to be pasted into a terminal.
 */
export function JoinTab({ runId, join }: { runId: string; join: JoinMutation }) {
  const { mutate, isIdle } = join
  const asked = useRef(false)
  useEffect(() => {
    if (asked.current || !isIdle) return
    asked.current = true
    mutate({ id: runId })
  }, [mutate, isIdle, runId])

  if (join.isError) return <ErrorAlert error={join.error} title="Could not get the join command" onRetry={() => join.mutate({ id: runId })} />
  if (!join.data) return <Skeleton className="h-20" />
  const { argv, cwd, env_names } = join.data
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">Run this in a terminal to attach to the run. Secret values are redacted by the server.</p>
      <CommandBlock command={joinCommand(argv)} cwd={shellQuote(cwd)} />
      {env_names.length > 0 && (
        <p className="text-sm">
          sets: <code className="font-mono text-xs">{env_names.join(', ')}</code>
        </p>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          title="Records a join_command_issued audit event each time"
          disabled={join.isPending}
          onClick={() => join.mutate({ id: runId })}
        >
          Issue again
        </Button>
      </div>
    </div>
  )
}
