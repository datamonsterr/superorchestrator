import { describe, expect, it } from 'vitest'
import type { Event, EventType } from '@/api/types'
import { briefFrom, EVENT_GROUPS, groupOf, joinCommand, shellQuote, summarize } from './data'

const ev = (type: EventType, data: unknown = {}): Event => ({ id: 'e', session_id: 's', run_id: 'r', ts: '2026-01-01T12:00:00Z', type, data })

describe('run panel data helpers', () => {
  it('groups every event type into one of the four groups', () => {
    const ids = EVENT_GROUPS.map((g) => g.id)
    expect(groupOf('state_changed')).toBe('lifecycle')
    expect(groupOf('tool_call_started')).toBe('activity')
    expect(groupOf('message_sent')).toBe('messages')
    expect(groupOf('stall_detected')).toBe('stalls')
    expect(ids).toContain(groupOf('escalated'))
  })
  it('summarises events', () => {
    expect(summarize(ev('state_changed', { from: 'running', to: 'waiting' }))).toBe('running to waiting')
    expect(summarize(ev('failover', { from: 'a', to: 'b', reason: 'quota' }))).toBe('a to b (quota)')
    expect(summarize(ev('tool_call_finished', { name: 'bash', ok: false }))).toBe('bash failed')
    expect(summarize(ev('usage'))).toBe('')
  })
  it('extracts the brief and options from run_queued', () => {
    const b = briefFrom([ev('nudge_sent'), ev('run_queued', { brief: 'Do it', isolation: 'worktree', checks: ['lint'], output_schema: { type: 'object' } })])!
    expect(b.brief).toBe('Do it')
    expect(b.isolation).toBe('worktree')
    expect(b.options).toEqual([['isolation', 'worktree'], ['checks', ['lint']]])
    expect(b.outputSchema).toEqual({ type: 'object' })
    expect(briefFrom([ev('nudge_sent')])).toBeNull()
  })
  it('quotes shell arguments', () => {
    expect(shellQuote('--flag')).toBe('--flag')
    expect(shellQuote('a b')).toBe("'a b'")
    expect(shellQuote("it's")).toBe(`'it'\\''s'`)
    expect(shellQuote('')).toBe("''")
    expect(shellQuote('***')).toBe("'***'")
    expect(joinCommand(['codex', '--resume', 'abc 1'])).toBe("codex --resume 'abc 1'")
  })
})
