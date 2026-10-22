import { cn } from '@/lib/utils'
import { CopyButton } from './CopyButton'

interface Props {
  command: string
  /** Shown as `cd <cwd> && <command>` when set. */
  cwd?: string
  className?: string
}

export function CommandBlock({ command, cwd, className }: Props) {
  const full = cwd ? `cd ${cwd} && ${command}` : command
  return (
    <div className={cn('flex items-start gap-2 rounded-md border bg-muted/50 p-2', className)}>
      <pre className="min-w-0 flex-1 overflow-x-auto whitespace-pre-wrap break-all font-mono text-sm select-all">
        <code>{full}</code>
      </pre>
      <CopyButton value={full} label="Copy command" />
    </div>
  )
}
