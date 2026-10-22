import { AlertCircle } from 'lucide-react'
import type { RunViewPublic } from '@/api/types'
import { JsonView } from '@/components/common/JsonView'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { briefFrom } from './data'
import { useAllRunEvents } from './hooks'

const ERROR_TEXT: Record<string, string> = {
  quota: 'The account ran out of quota.',
  rate_limit: 'The account was rate limited.',
  auth: 'The account is not signed in or its login expired.',
  other: 'The run failed for another reason.',
}

export function ResultTab({ run }: { run: RunViewPublic }) {
  const events = useAllRunEvents(run.run_id)
  const schemaSet = briefFrom(events.data ?? [])?.outputSchema !== undefined
  return (
    <div className="flex flex-col gap-4">
      {run.state === 'failed' && (
        <Alert variant="destructive" role="alert">
          <AlertCircle aria-hidden />
          <AlertTitle>Run failed{run.error_kind ? `: ${run.error_kind}` : ''}</AlertTitle>
          <AlertDescription>{ERROR_TEXT[run.error_kind ?? 'other']}</AlertDescription>
        </Alert>
      )}
      {schemaSet && <p className="text-sm text-muted-foreground">An output schema was set for this run; the result is checked against it before the run finishes.</p>}
      {run.result === undefined || run.result === null ? (
        <p className="text-sm text-muted-foreground">{run.state === 'succeeded' || run.state === 'failed' ? 'No result was recorded.' : 'No result yet.'}</p>
      ) : (
        <JsonView value={run.result} label="Result" defaultOpen />
      )}
    </div>
  )
}
