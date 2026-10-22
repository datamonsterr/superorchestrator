import { Badge } from '@/components/ui/badge'
import type { WorkflowState } from '@/api/types'

const GLYPH: Record<string, string> = { queued: '…', running: '▶', waiting: '…', stalled: '!', cached: '≡', succeeded: '✓', failed: '✗', cancelled: '–' }

/** Text plus glyph, so state is never colour-only. */
export function WorkflowStateBadge({ state }: { state: WorkflowState }) {
  return (
    <Badge variant={state === 'failed' ? 'destructive' : 'outline'} className="gap-1.5 capitalize">
      <span aria-hidden className="font-mono text-[10px]">
        {GLYPH[state] ?? '?'}
      </span>
      {state}
    </Badge>
  )
}
