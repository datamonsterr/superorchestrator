import type { RouteResult } from '@/api/types'

/** Short names for the decider's five complexity anchors, in score order (see `COMPLEXITY_ANCHORS`). */
const ANCHORS = ['0 trivial', '1 simple', '2 moderate', '3 complex', '4 open-ended']

/** The decider's per-anchor probabilities keyed `"0".."4"`, relabelled for display. */
export function anchorProbabilities(p: RouteResult['probabilities']): Record<string, number> {
  return Object.fromEntries(Object.entries(p).map(([k, v]) => [ANCHORS[Number(k)] ?? k, v]))
}

/** `complexity 55.0 · level mid`, or an empty string when the route carries neither. */
export function complexityText(r: Pick<RouteResult, 'complexity' | 'level'>): string {
  return [r.complexity != null ? `complexity ${r.complexity.toFixed(1)}` : '', r.level ? `level ${r.level}` : ''].filter(Boolean).join(' · ')
}
