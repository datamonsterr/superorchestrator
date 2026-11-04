import { Badge } from '@/components/ui/badge'

export function ReservedBadge() {
  return <Badge className="ml-2" variant="outline" title="Kept for the orchestrator; auto routing skips it">reserved</Badge>
}
