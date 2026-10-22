import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { endpoints } from '@/api/endpoints'

/** Token prompt for non-loopback binds. The token is posted once and never stored in JS. */
export function TokenPage() {
  const [token, setToken] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const navigate = useNavigate()
  const qc = useQueryClient()

  async function submit(e: FormEvent) {
    e.preventDefault()
    setPending(true)
    setError(null)
    try {
      await endpoints.postAuth(token)
      setToken('')
      await qc.invalidateQueries()
      void navigate('/dashboard', { replace: true })
    } catch (err) {
      setError(err)
    } finally {
      setPending(false)
    }
  }

  return (
    <main className="flex min-h-svh items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <h1 className="text-lg font-semibold">Superorchestrator</h1>
          <CardTitle className="sr-only">Sign in</CardTitle>
          <CardDescription>Enter the token printed by `superorchestrator ui`.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="flex flex-col gap-3">
            <label className="flex flex-col gap-1.5 text-sm">
              Access token
              <Input type="password" value={token} onChange={(e) => setToken(e.target.value)} autoComplete="off" required autoFocus />
            </label>
            {error != null && <ErrorAlert error={error} title="Could not sign in" />}
            <Button type="submit" disabled={pending || token === ''}>
              {pending ? 'Checking…' : 'Continue'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  )
}
