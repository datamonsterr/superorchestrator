import { Badge } from '@/components/ui/badge'

/** Text label plus a dot, so status is never colour-only. */
export function StatusBadge({ label, dot, ...data }: { label: string; dot: string } & Record<`data-${string}`, string>) {
  return (
    <Badge variant="outline" className="gap-1.5" {...data}>
      <span aria-hidden className={`size-2 rounded-full ${dot}`} />
      {label}
    </Badge>
  )
}
