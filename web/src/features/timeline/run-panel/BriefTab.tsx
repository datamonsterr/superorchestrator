import { ErrorAlert } from '@/components/common/ErrorAlert'
import { Skeleton } from '@/components/ui/skeleton'
import { briefFrom } from './data'
import { useAllRunEvents } from './hooks'

const show = (v: unknown) => (typeof v === 'string' ? v : JSON.stringify(v))

/** The prompt and spawn options, from the `run_queued` event. */
export function BriefTab({ runId }: { runId: string }) {
  const events = useAllRunEvents(runId)
  if (events.isPending) return <Skeleton className="h-24" />
  if (events.isError) return <ErrorAlert error={events.error} onRetry={() => void events.refetch()} />
  const brief = briefFrom(events.data)
  if (!brief) return <p className="text-sm text-muted-foreground">No brief was recorded for this run.</p>
  return (
    <div className="flex flex-col gap-4">
      <section aria-labelledby="brief-h">
        <h3 id="brief-h" className="mb-1 text-sm font-medium">
          Brief
        </h3>
        <pre className="max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-sans text-sm whitespace-pre-wrap">{brief.brief ?? 'No brief text.'}</pre>
      </section>
      {brief.options.length > 0 && (
        <section aria-labelledby="opts-h">
          <h3 id="opts-h" className="mb-1 text-sm font-medium">
            Spawn options
          </h3>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            {brief.options.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-muted-foreground">{k}</dt>
                <dd className="font-mono text-xs break-all">{show(v)}</dd>
              </div>
            ))}
          </dl>
        </section>
      )}
    </div>
  )
}
