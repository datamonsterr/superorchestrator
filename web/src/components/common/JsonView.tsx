import { ChevronRight } from 'lucide-react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { stringifyJson } from '@/lib/format'
import { cn } from '@/lib/utils'

interface Props {
  value: unknown
  label?: string
  defaultOpen?: boolean
  className?: string
}

/** Collapsible pretty-printed JSON. Rendered as a text node only (design 7.5). */
export function JsonView({ value, label = 'JSON', defaultOpen = false, className }: Props) {
  return (
    <Collapsible defaultOpen={defaultOpen} className={cn('rounded-md border', className)}>
      <CollapsibleTrigger className="group flex w-full items-center gap-1 px-2 py-1.5 text-sm font-medium">
        <ChevronRight aria-hidden className="size-4 transition-transform group-data-[state=open]:rotate-90" />
        {label}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="max-h-96 overflow-auto border-t bg-muted/40 p-2 font-mono text-xs">{stringifyJson(value)}</pre>
      </CollapsibleContent>
    </Collapsible>
  )
}
