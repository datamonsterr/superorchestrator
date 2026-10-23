import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { useLocation, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { endpoints } from '@/api/endpoints'

/** Exchanges the CLI fragment or a manually entered token for the session cookie. */
export function TokenPage() {
  const [token, setToken] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const navigate = useNavigate()
  const location = useLocation()
  const exchanged = useRef(false)
  const qc = useQueryClient()

  const exchange = useCallback(async (secret: string) => {
    setPending(true)
    setError(null)
    try {
      await endpoints.postAuth(secret)
      setToken('')
      await qc.invalidateQueries()
      void navigate('/dashboard', { replace: true })
    } catch (err) {
      setError(err)
    } finally {
      setPending(false)
    }
  }, [navigate, qc])

  useEffect(() => {
    const secret = new URLSearchParams(location.hash.slice(1)).get('token')
    if (!secret || exchanged.current) return
    exchanged.current = true
    // Remove the secret from the current history entry before any network request.
    window.history.replaceState(window.history.state, '', window.location.pathname + window.location.search)
    void navigate({ pathname: '/token', search: location.search, hash: '' }, { replace: true })
    void exchange(secret)
  }, [location.hash, location.search, navigate, exchange])

  function submit(e: FormEvent) {
    e.preventDefault()
    void exchange(token)
  }

  return (
    <main className="flex min-h-svh items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <h1 className="text-lg font-semibold">Superorchestrator</h1>
          <CardTitle className="sr-only">Sign in</CardTitle>
          <CardDescription>Open the sign-in URL printed by `superorchestrator ui`, or enter your access token.</CardDescription>
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
