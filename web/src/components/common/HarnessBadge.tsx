import { Badge } from '@/components/ui/badge'
import type { Harness } from '@/api/types'

const LABEL: Record<Harness, string> = { codex: 'Codex', agy: 'Antigravity', opencode: 'OpenCode', claude: 'Claude' }

export function HarnessBadge({ harness, className }: { harness: Harness; className?: string }) {
  return (
    <Badge variant="secondary" className={className} data-harness={harness}>
      {LABEL[harness] ?? harness}
    </Badge>
  )
}
