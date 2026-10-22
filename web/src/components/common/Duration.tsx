import { useState } from 'react'
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
  const [mounted] = useState(() => Date.now())
  let value = ms
  if (value === undefined && start != null) {
    value = (end != null ? new Date(end).getTime() : (now ?? mounted)) - new Date(start).getTime()
  }
  if (value === undefined || Number.isNaN(value)) return <span className={className}>-</span>
  return (
    <span className={className} title={`${Math.round(value)} ms`}>
      {formatDuration(value)}
    </span>
  )
}
