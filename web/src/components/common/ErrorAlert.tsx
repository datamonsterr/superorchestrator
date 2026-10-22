import { AlertCircle } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { ApiError } from '@/api/client'
import { errorMessage } from '@/lib/errors'

interface Props {
  error: unknown
  title?: string
  onRetry?: () => void
}

/** Blocking error with message, API code and an optional Retry. */
export function ErrorAlert({ error, title = 'Something went wrong', onRetry }: Props) {
  const code = error instanceof ApiError && error.code !== 0 ? error.code : undefined
  return (
    <Alert variant="destructive" role="alert">
      <AlertCircle aria-hidden />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>
        <p>{errorMessage(error)}</p>
        {code !== undefined && <p className="font-mono text-xs">code {code}</p>}
        {onRetry && (
          <Button type="button" variant="outline" size="sm" className="mt-2" onClick={onRetry}>
            Retry
          </Button>
        )}
      </AlertDescription>
    </Alert>
  )
}
