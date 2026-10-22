import { isRouteErrorResponse, useNavigate, useRouteError } from 'react-router'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { ErrorAlert } from '@/components/common/ErrorAlert'

/** Per-route fallback so one page crashing does not take navigation down with it. */
export function RouteErrorBoundary() {
  const error = useRouteError()
  const navigate = useNavigate()
  const err = isRouteErrorResponse(error) ? new Error(`${error.status} ${error.statusText}`) : error
  return (
    <Card className="mx-auto max-w-xl" role="alert">
      <CardHeader>
        <CardTitle>This page failed to load</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <ErrorAlert error={err} title="Unexpected error" />
        <div className="flex gap-2">
          <Button onClick={() => window.location.reload()}>Reload</Button>
          <Button variant="outline" onClick={() => void navigate('/dashboard')}>
            Go to dashboard
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
