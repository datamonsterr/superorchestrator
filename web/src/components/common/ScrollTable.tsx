import type { ReactNode } from 'react'
import { ScrollArea, ScrollBar } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'

/**
 * Table frame for narrow screens (design 7.3): the table scrolls sideways inside a ScrollArea and the
 * first column stays put. Tables inside must not scroll themselves, so give them
 * `containerClassName="overflow-visible"`; the shadcn Table does that here via the selector below.
 */
export function ScrollTable({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <ScrollArea
      className={cn(
        'w-full rounded-md border',
        '[&_[data-slot=table-container]]:overflow-visible',
        '[&_tr>:first-child]:sticky [&_tr>:first-child]:left-0 [&_tr>:first-child]:z-[1] [&_tr>:first-child]:bg-background',
        className,
      )}
    >
      {children}
      <ScrollBar orientation="horizontal" />
    </ScrollArea>
  )
}
