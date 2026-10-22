import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { server } from '@/test/server'
import { LARGE_SESSION_ID } from '@/test/fixtures/large'
import { endpoints } from '../endpoints'
import { createHandlers, paginate } from './handlers'
const GOOD = '---\nname: new-agent\ndescription: d\n---\nbody\n'

describe('base handlers cover every endpoint in design 2.1', () => {
  const calls: [string, () => Promise<unknown>][] = [
    ['getStatus', () => endpoints.getStatus()],
    ['getDashboard', () => endpoints.getDashboard()],
    ['getDailyUsage', () => endpoints.getDailyUsage(7)],
    ['getDoctor', () => endpoints.getDoctor()],
    ['getHarnesses', () => endpoints.getHarnesses()],
    ['listSessions', () => endpoints.listSessions()],
    ['getSession', () => endpoints.getSession('sess-1')],
    ['listSessionEvents', () => endpoints.listSessionEvents('sess-1')],
    ['getRun', () => endpoints.getRun('run-1')],
    ['listRunEvents', () => endpoints.listRunEvents('run-1')],
    ['getTranscript', () => endpoints.getTranscript('run-1')],
    ['getRunRoute', () => endpoints.getRunRoute('run-1')],
    ['listRunMessages', () => endpoints.listRunMessages('run-1')],
    ['sendMessage', () => endpoints.sendMessage('run-1', { text: 'hi' })],
    ['cancelRun', () => endpoints.cancelRun('run-1')],
    ['acceptRun', () => endpoints.acceptRun('run-2')],
    ['rejectRun', () => endpoints.rejectRun('run-2', 'no')],
    ['joinRun', () => endpoints.joinRun('run-1')],
    ['route', () => endpoints.route({ prompt: 'x' })],
    ['listRouteRecords', () => endpoints.listRouteRecords()],
    ['listAccounts', () => endpoints.listAccounts()],
    ['createAccount', () => endpoints.createAccount({ harness: 'codex', name: 'codex-acc2' })],
    ['importAccount', () => endpoints.importAccount({ harness: 'codex', name: 'codex-acc9', home: '/h' })],
    ['scanAccounts', () => endpoints.scanAccounts()],
    ['patchAccount', () => endpoints.patchAccount('codex-acc1', { cap: 3 })],
    ['removeAccount', () => endpoints.removeAccount('codex-acc1', true)],
    ['flagAccount', () => endpoints.flagAccount('agy-acc1', { hours: 5 })],
    ['startLogin', () => endpoints.startLogin('agy-acc1')],
    ['getLoginStatus', () => endpoints.getLoginStatus('agy-acc1')],
    ['getChoices', () => endpoints.getChoices()],
    ['putChoices', () => endpoints.putChoices({ choices: [], routing_instruction: '' })],
    ['getModels', () => endpoints.getModels()],
    ['refreshModels', () => endpoints.refreshModels()],
    ['listWorktrees', () => endpoints.listWorktrees()],
    ['pruneWorktrees', () => endpoints.pruneWorktrees({ dry_run: true })],
    ['listAgents', () => endpoints.listAgents()],
    ['createAgent', () => endpoints.createAgent({ name: 'new-agent', source: GOOD })],
    ['getAgent', () => endpoints.getAgent('reviewer')],
    ['putAgent', () => endpoints.putAgent('reviewer', { source: GOOD })],
    ['deleteAgent', () => endpoints.deleteAgent('reviewer')],
    ['listWorkflows', () => endpoints.listWorkflows()],
    ['listWorkflowRuns', () => endpoints.listWorkflowRuns()],
    ['getWorkflowRun', () => endpoints.getWorkflowRun('wf-1')],
    ['runWorkflow', () => endpoints.runWorkflow('ship-feature', { a: 1 })],
    ['getConfig', () => endpoints.getConfig()],
    ['patchConfig', () => endpoints.patchConfig({ runs: { stall_ms: 1 } })],
    ['postAuth', () => endpoints.postAuth('t')],
  ]

  it('exercises every function in `endpoints`', () => {
    expect(calls.map(([n]) => n).sort()).toEqual(Object.keys(endpoints).sort())
  })
  it.each(calls)('%s resolves against the fixtures', async (_name, call) => {
    await expect(call()).resolves.not.toThrow()
  })
})

describe('behaviour', () => {
  it('filters sessions by status and q', async () => {
    expect((await endpoints.listSessions({ status: 'failed' })).items.map((s) => s.id)).toEqual(['sess-3'])
    expect((await endpoints.listSessions({ q: 'other' })).items.map((s) => s.id)).toEqual(['sess-2'])
  })
  it('returns 404 and 409 error bodies', async () => {
    await expect(endpoints.getRun('missing')).rejects.toMatchObject({ status: 404, code: -32003 })
    await expect(endpoints.createAgent({ name: 'reviewer', source: GOOD })).rejects.toMatchObject({ status: 409 })
  })
  it('clamps and pages with an opaque cursor', () => {
    const all = Array.from({ length: 2500 }, (_, i) => i)
    const p1 = paginate(all, new URL('http://x/?limit=5000'))
    expect(p1.items).toHaveLength(1000)
    expect(p1.next_cursor).toBe('1000')
    expect(paginate(all, new URL('http://x/?cursor=2000&limit=1000')).next_cursor).toBeNull()
  })
})

describe('mock sign-in mode follows the harness the account was created with', () => {
  it('returns a command for a claude account whatever its name, and a device code for the others', async () => {
    await endpoints.createAccount({ harness: 'claude', name: 'work-a' })
    await endpoints.createAccount({ harness: 'codex', name: 'claude-lookalike' })
    expect(await endpoints.startLogin('work-a')).toMatchObject({ mode: 'manual', command: 'claude login' })
    expect(await endpoints.startLogin('claude-lookalike')).toMatchObject({ mode: 'device_auth' })
  })
})

describe('large session', () => {
  const small = { events: 2500, runs: 10, transcriptRows: 600, seed: 3 }
  it('serves events across pages, a session detail and runs', async () => {
    server.use(...createHandlers({ large: true, largeOptions: small }))
    const list = await endpoints.listSessions()
    expect(list.items[0].id).toBe(LARGE_SESSION_ID)
    const detail = await endpoints.getSession(LARGE_SESSION_ID)
    expect(detail.runs).toHaveLength(10)
    let cursor: string | null = null
    const ids: string[] = []
    do {
      const page: Awaited<ReturnType<typeof endpoints.listSessionEvents>> = await endpoints.listSessionEvents(LARGE_SESSION_ID, { after: cursor, limit: 1000 })
      ids.push(...page.items.map((e) => e.id))
      cursor = page.next_cursor
    } while (cursor)
    expect(ids).toHaveLength(2500)
    expect(new Set(ids).size).toBe(2500)
    expect((await endpoints.getRun(detail.runs[0].run_id)).run_id).toBe(detail.runs[0].run_id)
  })
  it('pages the transcript by byte offset', async () => {
    server.use(...createHandlers({ large: true, largeOptions: small }))
    const runId = (await endpoints.getSession(LARGE_SESSION_ID)).runs[0].run_id
    const first = await endpoints.getTranscript(runId, { offset: 0, limit: 500 })
    expect(first.lines).toHaveLength(500)
    expect(first.next_offset).not.toBeNull()
    const second = await endpoints.getTranscript(runId, { offset: first.next_offset!, limit: 500 })
    expect(second.lines).toHaveLength(100)
    expect(second.next_offset).toBeNull()
  })
  it('is routable by id even when not listed', async () => {
    const list = await endpoints.listSessions()
    expect(list.items.some((s) => s.id === LARGE_SESSION_ID)).toBe(false)
    server.use(http.get('*/api/v1/status', () => HttpResponse.json({})))
    await expect(endpoints.getSession(LARGE_SESSION_ID)).resolves.toMatchObject({ id: LARGE_SESSION_ID })
  })
})
