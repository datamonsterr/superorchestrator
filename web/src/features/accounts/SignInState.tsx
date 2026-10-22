import { Badge } from '@/components/ui/badge'
import type { SignInState as State } from '@/api/types'

const S: Record<State, { label: string; dot: string }> = {
  signed_in: { label: 'Signed in', dot: 'bg-green-600 dark:bg-green-500' },
  signed_out: { label: 'Signed out', dot: 'bg-zinc-400' },
  unknown: { label: 'Unknown', dot: 'bg-amber-500' },
}

/** Text plus a dot: sign-in is never colour-only. */
export function SignInState({ state }: { state: State }) {
  const s = S[state]
  return (
    <Badge variant="outline" className="gap-1.5" data-signin={state}>
      <span aria-hidden className={`size-2 rounded-full ${s.dot}`} />
      {s.label}
    </Badge>
  )
}
