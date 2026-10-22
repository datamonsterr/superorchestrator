import { StatusBadge } from './StatusBadge'
import type { SignInState as State } from '@/api/types'

const S: Record<State, { label: string; dot: string }> = {
  signed_in: { label: 'Signed in', dot: 'bg-green-600 dark:bg-green-500' },
  signed_out: { label: 'Signed out', dot: 'bg-zinc-400' },
  unknown: { label: 'Unknown', dot: 'bg-amber-500' },
}

export function SignInState({ state }: { state: State }) {
  return <StatusBadge {...S[state]} data-signin={state} />
}
