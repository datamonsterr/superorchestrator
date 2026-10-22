import { Badge } from '@/components/ui/badge'
import type { SecretSource } from '@/api/types'

/**
 * Where a secret comes from. Only the origin and its name (an env var name or a file path) are shown;
 * the API never sends the value and this component accepts none.
 */
export function KeySource({ label, source }: { label: string; source: SecretSource }) {
  const set = source.source !== 'unset'
  return (
    <div className="flex flex-col gap-1 rounded-md border p-3 text-sm" data-testid={`key-source-${label}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{label}</span>
        <Badge variant={set ? 'secondary' : 'outline'}>{source.source === 'env' ? 'Environment variable' : source.source === 'file' ? 'File' : 'Unset'}</Badge>
        {source.name && <span className="font-mono text-xs">{source.name}</span>}
      </div>
      <p className="text-muted-foreground">Key set: {set ? 'yes' : 'no'}</p>
    </div>
  )
}
