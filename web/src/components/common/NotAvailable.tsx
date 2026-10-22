import { PlugZap } from 'lucide-react'
import { EmptyState } from './EmptyState'

/** Empty state for a feature the connected supervisor does not serve yet. */
export function NotAvailable({ what }: { what: string }) {
  return (
    <EmptyState
      icon={PlugZap}
      title="Not available on this supervisor"
      description={`${what} is not served by the connected supervisor. Update superorchestrator to a version that includes it.`}
    />
  )
}
