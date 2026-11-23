import { useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { formatDuration } from '@/lib/format'
import type { AccountFlag } from '@/api/types'

interface Props {
  flag: AccountFlag
  /** Fixed "now" (epoch ms) for tests; otherwise the clock ticks every 30 s. */
  now?: number
}

/** "cooling down claude until Nov 4, 10:30 AM (in 2h 10m)": the end time of a cooldown, with the time left. */
export function CooldownBadge({ flag, now }: Props) {
  const [tick, setTick] = useState(() => Date.now())
  useEffect(() => {
    if (now !== undefined) return
    const id = setInterval(() => setTick(Date.now()), 30_000)
    return () => clearInterval(id)
  }, [now])
  const end = new Date(flag.until)
  if (Number.isNaN(end.getTime())) return null
  const at = end.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
  const left = end.getTime() - (now ?? tick)
  const rest = left > 0 ? `in ${formatDuration(left)}` : 'ending now'
  return (
    <Badge variant="destructive" title={flag.reason ?? undefined} data-cooldown={flag.pool ?? 'all'}>
      cooling down{flag.pool ? ` ${flag.pool}` : ''} until <time dateTime={end.toISOString()}>{at}</time> ({rest})
    </Badge>
  )
}
