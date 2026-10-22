import { useEffect, useState } from 'react'
import { formatDuration } from '@/lib/format'

interface Props {
  /** Milliseconds, or give start and end. An open range ends at `now`. */
  ms?: number
  start?: string | number | Date | null
  end?: string | number | Date | null
  now?: number
  className?: string
}

export function Duration({ ms, start, end, now, className }: Props) {
  const [tick, setTick] = useState(() => Date.now())
  const open = ms === undefined && start != null && end == null && now === undefined
  // An open range counts up once a second.
  useEffect(() => {
    if (!open) return
    const id = setInterval(() => setTick(Date.now()), 1000)
    return () => clearInterval(id)
  }, [open])
  let value = ms
  if (value === undefined && start != null) {
    value = (end != null ? new Date(end).getTime() : (now ?? tick)) - new Date(start).getTime()
  }
  if (value === undefined || Number.isNaN(value)) return <span className={className}>-</span>
  return (
    <span className={className} title={`${Math.round(value)} ms`}>
      {formatDuration(value)}
    </span>
  )
}
