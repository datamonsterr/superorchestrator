import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, ExternalLink, Loader2 } from 'lucide-react'
import { toast } from 'sonner'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { CommandBlock } from '@/components/common/CommandBlock'
import { CopyButton } from '@/components/common/CopyButton'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { qk, useAccounts, useSignInStatus, useStartLogin } from '@/api/queries'
import type { AccountView, SignInStart } from '@/api/types'

interface Props {
  /** Account to sign in; the dialog is open while this is set. */
  name: string | null
  onClose: () => void
}

/** Only http(s) targets become links; anything else (javascript:, data:) is shown as text. */
function isWebUrl(url: string): boolean {
  try {
    const p = new URL(url).protocol
    return p === 'https:' || p === 'http:'
  } catch {
    return false
  }
}

/** mm:ss until `expiresAt`, or null when it has passed. */
function useCountdown(expiresAt: string | null): string | null {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!expiresAt) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [expiresAt])
  if (!expiresAt) return null
  const end = new Date(expiresAt).getTime()
  if (!Number.isFinite(end)) return 'unknown' // unparseable time: do not claim it expired
  const left = Math.max(0, Math.floor((end - now) / 1000))
  if (left === 0) return null
  return `${String(Math.floor(left / 60)).padStart(2, '0')}:${String(left % 60).padStart(2, '0')}`
}

function SignInBody({ name, onClose }: { name: string; onClose: () => void }) {
  const qc = useQueryClient()
  const start = useStartLogin()
  const accounts = useAccounts()
  const [info, setInfo] = useState<SignInStart | null>(null)
  const startedFor = useRef<string | null>(null)
  // Sign-in state when the dialog opened, to tell a fresh sign-in (SSE `account` event) from an old one.
  const [wasSignedIn] = useState(() => qc.getQueryData<AccountView[]>(qk.accounts())?.find((a) => a.name === name)?.signed_in === 'signed_in')
  const [accountsKnown] = useState(() => qc.getQueryData(qk.accounts()) !== undefined)

  const run = () => {
    // Drop a stale expired/failed status so polling restarts cleanly.
    qc.removeQueries({ queryKey: qk.signin(name) })
    setInfo(null)
    start.mutate({ name }, { onSuccess: (d) => setInfo(d) })
  }
  useEffect(() => {
    if (startedFor.current === name) return
    startedFor.current = name
    run()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- starts once per account
  }, [name])

  const countdown = useCountdown(info?.mode === 'device_auth' ? info.expires_at : null)
  const codeExpired = info?.mode === 'device_auth' && countdown === null
  const status = useSignInStatus(name, !!info && !codeExpired)
  const signedInNow = accounts.data?.find((a) => a.name === name)?.signed_in === 'signed_in'
  const done = status.data?.state === 'done' || (!!info && accountsKnown && !wasSignedIn && signedInNow)
  const failed = status.data?.state === 'failed' || status.data?.state === 'expired'

  const notified = useRef(false)
  useEffect(() => {
    if (!done || notified.current) return
    notified.current = true
    toast.success(`${name} signed in`)
    void Promise.all([qk.accounts(), qk.harnesses(), qk.dashboard()].map((queryKey) => qc.invalidateQueries({ queryKey })))
    onClose()
  }, [done, name, qc, onClose])

  const retry = () => {
    notified.current = false
    run()
  }

  return (
    <div className="flex flex-col gap-4">
      {start.isError && <ErrorAlert error={start.error} title="Could not start sign-in" onRetry={retry} />}
      {start.isPending && (
        <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
          <Loader2 aria-hidden className="size-4 animate-spin" /> Starting sign-in…
        </p>
      )}

      {info?.mode === 'device_auth' && (
        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-2">
            {isWebUrl(info.url) ? (
              <Button asChild variant="outline" size="sm">
                <a href={info.url} target="_blank" rel="noreferrer">
                  <ExternalLink aria-hidden /> Open sign-in page
                </a>
              </Button>
            ) : null}
            <span className="min-w-0 truncate text-sm text-muted-foreground" title={info.url}>
              {info.url}
            </span>
          </div>
          <div className="flex items-center gap-3 rounded-md border bg-muted/50 p-3">
            <code aria-label="One-time code" className="flex-1 text-center font-mono text-3xl font-semibold tracking-[0.25em]">
              {info.code}
            </code>
            <CopyButton value={info.code} label="Copy code" />
          </div>
          <p className="text-sm text-muted-foreground">{countdown === 'unknown' ? 'Expiry time unknown.' : countdown ? `Expires in ${countdown}` : 'The code has expired.'}</p>
        </div>
      )}

      {info?.mode === 'manual' && (
        <div className="flex flex-col gap-2">
          <CommandBlock command={info.command} cwd={info.cwd ?? undefined} />
          <p className="text-sm text-muted-foreground">Run this in a terminal, then come back.</p>
        </div>
      )}

      {info && !done && !failed && !codeExpired && (
        <p className="flex items-center gap-2 text-sm" role="status">
          <Loader2 aria-hidden className="size-4 animate-spin" /> Waiting for sign-in
        </p>
      )}

      {done && (
        <Alert>
          <CheckCircle2 aria-hidden />
          <AlertTitle>Signed in</AlertTitle>
          <AlertDescription>The credential file exists for {name}.</AlertDescription>
        </Alert>
      )}

      {(failed || codeExpired) && (
        <Alert variant="destructive" role="alert">
          <AlertTitle>{status.data?.state === 'failed' ? 'Sign-in failed' : 'Sign-in expired'}</AlertTitle>
          <AlertDescription>
            <p>{status.data?.message ?? 'The sign-in did not finish in time.'}</p>
            <Button type="button" variant="outline" size="sm" className="mt-2" onClick={retry}>
              Try again
            </Button>
          </AlertDescription>
        </Alert>
      )}
    </div>
  )
}

export function SignInDialog({ name, onClose }: Props) {
  return (
    <Dialog open={name !== null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Sign in{name ? ` ${name}` : ''}</DialogTitle>
          <DialogDescription>Finish the sign-in; this window updates by itself.</DialogDescription>
        </DialogHeader>
        {name && <SignInBody key={name} name={name} onClose={onClose} />}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
