import { Skeleton } from '@/components/ui/skeleton'

export function PageFallback() {
  return (
    <div role="status" aria-busy="true" aria-label="Loading" className="flex flex-col gap-3 p-6">
      <Skeleton className="h-8 w-48" />
      <Skeleton className="h-40 w-full" />
    </div>
  )
}
