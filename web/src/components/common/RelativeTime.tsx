import { useEffect, useState } from 'react'
import { formatRelative } from '@/lib/format'

interface Props {
  value: string | number | Date | null | undefined
  /** Fixed "now" (epoch ms) for tests and stories; otherwise the clock ticks every 30 s. */
  now?: number
  className?: string
}

export function RelativeTime({ value, now, className }: Props) {
  const [tick, setTick] = useState(() => Date.now())
  useEffect(() => {
    if (now !== undefined) return
    const id = setInterval(() => setTick(Date.now()), 30_000)
    return () => clearInterval(id)
  }, [now])
  if (value == null) return <span className={className}>-</span>
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return <span className={className}>unknown</span>
  return (
    <time dateTime={date.toISOString()} title={date.toLocaleString()} className={className}>
      {formatRelative(date, now ?? tick)}
    </time>
  )
}
