import { Badge } from '@/components/ui/badge'
import type { Harness } from '@/api/types'

const LABEL: Record<Harness, string> = {
  codex: 'Codex', agy: 'Antigravity', opencode: 'OpenCode', claude: 'Claude',
  copilot: 'Copilot', gemini: 'Gemini', kimi: 'Kimi', cursor: 'Cursor', qwen: 'Qwen',
}
const COLOR: Record<Harness, string> = {
  codex: 'text-foreground', agy: 'text-sky-700 dark:text-sky-300', opencode: 'text-violet-700 dark:text-violet-300', claude: 'text-orange-700 dark:text-orange-300',
  copilot: 'text-blue-700 dark:text-blue-300', gemini: 'text-emerald-700 dark:text-emerald-300', kimi: 'text-cyan-700 dark:text-cyan-300', cursor: 'text-indigo-700 dark:text-indigo-300', qwen: 'text-rose-700 dark:text-rose-300',
}

export function HarnessBadge({ harness, className }: { harness: Harness; className?: string }) {
  return (
    <Badge variant="secondary" className={[COLOR[harness], className].filter(Boolean).join(' ')} data-harness={harness}>
      {LABEL[harness] ?? harness}
    </Badge>
  )
}
