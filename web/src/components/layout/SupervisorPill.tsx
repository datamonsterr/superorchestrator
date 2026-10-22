import { Badge } from '@/components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useStatus } from '@/api/queries'
import { useStreamState } from '@/api/stream'
import { formatDuration } from '@/lib/format'
import { cn } from '@/lib/utils'

export type SupervisorHealth = 'up' | 'reconnecting' | 'unreachable' | 'unknown'

/** Red when /status failed (after one retry), amber when the stream is down, green otherwise. */
export function SupervisorPill() {
  const status = useStatus()
  const stream = useStreamState()
  const health: SupervisorHealth = status.isError ? 'unreachable' : !status.data ? 'unknown' : stream.polling ? 'reconnecting' : 'up'
  const text = {
    up: `up ${formatDuration((status.data?.uptime_s ?? 0) * 1000)}`,
    reconnecting: 'reconnecting',
    unreachable: 'unreachable',
    unknown: 'checking',
  }[health]
  const dot = { up: 'bg-green-600', reconnecting: 'bg-amber-500', unreachable: 'bg-red-600', unknown: 'bg-muted-foreground' }[health]
  return (
    <>
      {/* Announced only when health changes, not on every uptime tick. */}
      <span role="status" className="sr-only" data-health={health}>
        {`Supervisor ${health}`}
      </span>
      <Tooltip>
      <TooltipTrigger asChild>
        <Badge variant="outline" className="gap-1.5" data-health={health}>
          <span aria-hidden className={cn('size-2 rounded-full', dot)} />
          {text}
        </Badge>
      </TooltipTrigger>
      <TooltipContent>{status.data ? `pid ${status.data.pid}, version ${status.data.version}` : 'No status yet'}</TooltipContent>
      </Tooltip>
    </>
  )
}
