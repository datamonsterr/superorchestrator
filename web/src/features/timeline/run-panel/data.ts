import type { Event, EventType } from '@/api/types'
import { asString, isTerminalState } from '../model'

export const isTerminal = isTerminalState

/** Event type groups for the Events tab filter (design 4.9). */
export type EventGroup = 'lifecycle' | 'activity' | 'messages' | 'stalls'
export const EVENT_GROUPS: { id: EventGroup; label: string }[] = [
  { id: 'lifecycle', label: 'Lifecycle' },
  { id: 'activity', label: 'Activity' },
  { id: 'messages', label: 'Messages' },
  { id: 'stalls', label: 'Stalls' },
]
const GROUP_OF: Record<EventType, EventGroup> = {
  run_queued: 'lifecycle', run_started: 'lifecycle', state_changed: 'lifecycle', run_finished: 'lifecycle',
  attempt_started: 'lifecycle', failover: 'lifecycle', worktree_created: 'lifecycle', worktree_removed: 'lifecycle',
  review_accepted: 'lifecycle', review_rejected: 'lifecycle',
  route_decision: 'activity', tool_call_started: 'activity', tool_call_finished: 'activity', assistant_message: 'activity',
  progress: 'activity', usage: 'activity', check_result: 'activity',
  message_sent: 'messages', question_asked: 'messages', question_answered: 'messages', user_joined: 'messages',
  user_left: 'messages', join_command_issued: 'messages',
  stall_detected: 'stalls', nudge_sent: 'stalls', escalated: 'stalls',
}
export const groupOf = (t: EventType): EventGroup => GROUP_OF[t]

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' ? (v as Record<string, unknown>) : {})
const str = asString

/** One-line summary of an event for the list. */
export function summarize(e: Event): string {
  const d = obj(e.data)
  switch (e.type) {
    case 'state_changed':
      return `${str(d.from) ?? '?'} to ${str(d.to) ?? '?'}`
    case 'run_finished':
      return str(d.status) ?? ''
    case 'run_queued':
      return str(d.brief)?.slice(0, 120) ?? ''
    case 'attempt_started':
      return str(d.account) ?? ''
    case 'failover':
      return `${str(d.from) ?? '?'} to ${str(d.to) ?? '?'}${str(d.reason) ? ` (${str(d.reason)})` : ''}`
    case 'tool_call_started':
    case 'tool_call_finished':
      return `${str(d.name) ?? 'tool'}${d.ok === false ? ' failed' : ''}`
    case 'message_sent':
      return `${str(d.kind) ?? 'message'} to ${str(d.to) ?? '?'}`
    case 'check_result':
      return `${str(d.name) ?? 'check'} ${d.ok === false ? 'failed' : 'passed'}`
    case 'stall_detected':
      return typeof d.idle_ms === 'number' ? `idle ${Math.round(d.idle_ms / 1000)}s` : ''
    case 'escalated':
      return str(d.reason) ?? ''
    case 'question_asked':
      return str(d.question) ?? str(d.text) ?? ''
    default:
      return str(d.text) ?? str(d.message) ?? ''
  }
}

export interface Brief {
  brief?: string
  /** Everything else in the `run_queued` data (agent, isolation, review, checks, timeout, ...). */
  options: [string, unknown][]
  outputSchema?: unknown
  isolation?: string
}

/** Brief and spawn options come from the `run_queued` event. */
export function briefFrom(events: readonly Event[]): Brief | null {
  const e = events.find((x) => x.type === 'run_queued')
  if (!e) return null
  const d = obj(e.data)
  const { brief, output_schema, ...rest } = d
  return {
    brief: str(brief) ?? str(d.prompt),
    options: Object.entries(rest).filter(([k]) => k !== 'prompt'),
    outputSchema: output_schema,
    isolation: str(d.isolation),
  }
}

const SAFE = /^[A-Za-z0-9_@%+=:,./-]+$/
/** POSIX single-quote escaping for display; `***` redactions stay readable. */
export function shellQuote(arg: string): string {
  if (arg === '') return "''"
  // A leading dash would be read as an option (`cd -P`), so it is quoted; flags in argv keep their plain form.
  if (SAFE.test(arg) && !arg.startsWith('-')) return arg
  return `'${arg.replace(/'/g, `'\\''`)}'`
}
const FLAG = /^--?[A-Za-z0-9][A-Za-z0-9_.-]*(=[A-Za-z0-9_@%+=:,./-]*)?$/
export const joinCommand = (argv: readonly string[]) => argv.map((a) => (FLAG.test(a) ? a : shellQuote(a))).join(' ')
