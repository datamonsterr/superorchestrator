import { http, HttpResponse, type RequestHandler } from 'msw'
import type { ApiErrorBody, Event, Harness, Page, TranscriptLine } from '../types'
import { LARGE_SESSION_ID, makeLargeSession, type LargeSession } from '@/test/fixtures/large'
import * as fx from './fixtures'

const B = '/api/v1'
const ok = { ok: true }

export function apiError(status: number, code: number, message: string, data?: unknown) {
  const body: ApiErrorBody = { error: { code, message, data } }
  return HttpResponse.json(body, { status })
}

/** Cursor = index of the next item, as a string. Opaque to clients. */
export function paginate<T>(all: T[], url: URL, max = 1000): Page<T> {
  const limit = Math.min(Number(url.searchParams.get('limit')) || 200, max)
  const start = Number(url.searchParams.get('cursor') ?? url.searchParams.get('after') ?? 0) || 0
  const items = all.slice(start, start + limit)
  const next = start + limit
  return { items, next_cursor: next < all.length ? String(next) : null }
}

export interface HandlerOptions {
  /** List the large session in `/sessions` (dev:mock ?large=1). It is always routable by id. */
  large?: boolean
  largeOptions?: Parameters<typeof makeLargeSession>[0]
}

/** Base handlers for every endpoint in design 2.1. Override in tests with `server.use`. */
export function createHandlers({ large = false, largeOptions }: HandlerOptions = {}): RequestHandler[] {
  let cached: LargeSession | null = null
  const getLarge = () => (cached ??= makeLargeSession({ seed: 1, ...largeOptions }))
  // Harness of accounts created through the mock API, by name.
  const createdHarness = new Map<string, Harness>()
  const harnessOf = (name: string): Harness | undefined => createdHarness.get(name) ?? fx.accounts.find((a) => a.name === name)?.harness
  const largeRunIds = () => new Set(getLarge().session.runs.map((r) => r.run_id))

  const sessionList = () => (large ? [{ ...getLarge().session, runs: undefined } as never, ...fx.sessions] : fx.sessions)

  return [
    http.get(`${B}/status`, () => HttpResponse.json(fx.status)),
    http.get(`${B}/dashboard`, () => HttpResponse.json(fx.dashboard)),
    http.get(`${B}/usage/daily`, ({ request }) => {
      const days = Number(new URL(request.url).searchParams.get('days')) || 30
      return HttpResponse.json(fx.daily.slice(-days))
    }),
    http.get(`${B}/doctor`, () => HttpResponse.json(fx.doctor)),
    http.get(`${B}/harnesses`, () => HttpResponse.json({ accounts: fx.accounts, choices: fx.choices.choices })),

    http.get(`${B}/sessions`, ({ request }) => {
      const url = new URL(request.url)
      const status = url.searchParams.get('status')
      const q = url.searchParams.get('q')?.toLowerCase()
      const list = sessionList().filter(
        (s) =>
          (!status || s.status === status) &&
          (!q || [s.client, s.cwd, s.id].some((v) => v.toLowerCase().includes(q)) || fx.runs.some((r) => s.id === 'sess-1' && (r.name ?? '').toLowerCase().includes(q))),
      )
      return HttpResponse.json(paginate(list, url, 200))
    }),
    http.get(`${B}/sessions/:id`, ({ params }) => {
      if (params.id === LARGE_SESSION_ID) return HttpResponse.json(getLarge().session)
      const s = fx.sessions.find((x) => x.id === params.id)
      if (!s) return apiError(404, -32003, `session ${String(params.id)} not found`)
      return HttpResponse.json(s.id === 'sess-1' ? fx.sessionDetail : { ...s, runs: [] })
    }),
    http.get(`${B}/sessions/:id/events`, ({ params, request }) => {
      const all: Event[] = params.id === LARGE_SESSION_ID ? getLarge().events : params.id === 'sess-1' ? fx.events : []
      return HttpResponse.json(paginate(all, new URL(request.url), 1000))
    }),

    http.get(`${B}/runs/:id`, ({ params }) => {
      const id = String(params.id)
      const run = fx.runs.find((r) => r.run_id === id) ?? getLarge().session.runs.find((r) => r.run_id === id)
      return run ? HttpResponse.json(run) : apiError(404, -32003, `run ${id} not found`)
    }),
    http.get(`${B}/runs/:id/events`, ({ params, request }) => {
      const id = String(params.id)
      const url = new URL(request.url)
      const type = url.searchParams.get('type')
      const pool = fx.runs.some((r) => r.run_id === id) ? fx.events : largeRunIds().has(id) ? getLarge().events : []
      return HttpResponse.json(paginate(pool.filter((e) => e.run_id === id && (!type || e.type === type)), url, 1000))
    }),
    http.get(`${B}/runs/:id/transcript`, ({ params, request }) => {
      const id = String(params.id)
      const url = new URL(request.url)
      const lines: TranscriptLine[] = largeRunIds().has(id) ? getLarge().transcript : fx.transcript
      const offset = Number(url.searchParams.get('offset')) || 0
      const limit = Math.min(Number(url.searchParams.get('limit')) || 200, 1000)
      let from = lines.findIndex((l) => l.offset >= offset)
      if (from < 0) from = lines.length
      const slice = lines.slice(from, from + limit)
      const nextLine = lines[from + limit]
      const last = lines[lines.length - 1]
      return HttpResponse.json({
        lines: slice,
        next_offset: nextLine ? nextLine.offset : null,
        total_bytes: last ? last.offset + last.text.length + 1 : 0,
      })
    }),
    http.get(`${B}/runs/:id/route`, () => HttpResponse.json(fx.routeResult)),
    http.get(`${B}/runs/:id/messages`, () => HttpResponse.json(fx.messages)),
    http.post(`${B}/runs/:id/messages`, () => HttpResponse.json(ok)),
    http.post(`${B}/runs/:id/cancel`, () => HttpResponse.json(ok)),
    http.post(`${B}/runs/:id/accept`, () => HttpResponse.json(ok)),
    http.post(`${B}/runs/:id/reject`, () => HttpResponse.json(ok)),
    http.post(`${B}/runs/:id/join`, () =>
      HttpResponse.json({ argv: ['codex', '--resume', 'abc123', '--api-key', '***'], cwd: '/work/wt/run-1', env_names: ['CODEX_HOME'] }),
    ),

    http.post(`${B}/route`, () => HttpResponse.json(fx.routeResult)),
    http.get(`${B}/routing/records`, ({ request }) => HttpResponse.json(paginate(fx.routeRecords, new URL(request.url), 200))),

    http.get(`${B}/accounts`, () => HttpResponse.json(fx.accounts)),
    http.post(`${B}/accounts`, async ({ request }) => {
      const b = (await request.json()) as { harness: Harness; name: string }
      createdHarness.set(b.name, b.harness)
      return HttpResponse.json({ ...fx.accounts[0], name: b.name, harness: b.harness, signed_in: 'signed_out', active_runs: 0, headroom: [] }, { status: 201 })
    }),
    http.post(`${B}/accounts/import`, async ({ request }) => {
      const b = (await request.json()) as { harness: never; name: string }
      return HttpResponse.json({ ...fx.accounts[0], name: b.name, harness: b.harness })
    }),
    http.get(`${B}/accounts/scan`, () => HttpResponse.json(fx.scanCandidates)),
    http.patch(`${B}/accounts/:name`, async ({ params, request }) => {
      const base = fx.accounts.find((a) => a.name === params.name)
      if (!base) return apiError(404, -32003, `account ${String(params.name)} not found`)
      return HttpResponse.json({ ...base, ...((await request.json()) as object) })
    }),
    http.delete(`${B}/accounts/:name`, () => HttpResponse.json(ok)),
    http.post(`${B}/accounts/:name/flag`, ({ params }) => HttpResponse.json(fx.accounts.find((a) => a.name === params.name) ?? fx.accounts[0])),
    http.post(`${B}/accounts/:name/login`, ({ params }) =>
      // Claude accounts sign in with a command; the others use a device code. The harness is what the
      // account was created with, never guessed from its name.
      harnessOf(String(params.name)) === 'claude'
        ? HttpResponse.json({ mode: 'manual', command: 'claude login', cwd: '/home/me' })
        : HttpResponse.json({ mode: 'device_auth', url: 'https://example.test/device', code: 'ABCD-1234', expires_at: new Date(Date.UTC(2026, 0, 1, 12, 15)).toISOString() }),
    ),
    http.get(`${B}/accounts/:name/login`, () => HttpResponse.json({ signed_in: 'signed_out', mode: 'device_auth', state: 'pending' })),

    http.get(`${B}/choices`, () => HttpResponse.json(fx.choices)),
    http.put(`${B}/choices`, async ({ request }) => HttpResponse.json(await request.json())),
    http.get(`${B}/models`, () => HttpResponse.json(fx.models)),
    http.post(`${B}/models/refresh`, () => HttpResponse.json(fx.models)),

    http.get(`${B}/worktrees`, () => HttpResponse.json(fx.worktrees)),
    http.post(`${B}/worktrees/prune`, async ({ request }) => {
      const b = (await request.json().catch(() => ({}))) as { ids?: string[] }
      const prunable = fx.worktrees.filter((w) => w.orphaned).map((w) => w.run_id)
      if (!b.ids) return HttpResponse.json({ removed: prunable, skipped: [] })
      return HttpResponse.json({ removed: b.ids.filter((id) => prunable.includes(id)), skipped: b.ids.filter((id) => !prunable.includes(id)) })
    }),

    http.get(`${B}/agents`, () => HttpResponse.json({ agents: fx.agentViews })),
    http.post(`${B}/agents`, async ({ request }) => {
      const b = (await request.json()) as { name: string; source: string }
      if (fx.agents.some((a) => a.name === b.name)) return apiError(409, -32002, `agent \`${b.name}\` already exists`)
      return agentWrite(b.source, 201)
    }),
    http.get(`${B}/agents/:name`, ({ params }) => {
      const a = fx.agents.find((x) => x.name === params.name)
      return a ? HttpResponse.json(a) : apiError(404, -32003, `no agent named \`${String(params.name)}\``)
    }),
    http.put(`${B}/agents/:name`, async ({ params, request }) => {
      const a = fx.agents.find((x) => x.name === params.name)
      if (!a) return apiError(404, -32003, `no agent named \`${String(params.name)}\``)
      if (a.read_only) return apiError(422, -32602, `\`${a.name}\` is a read-only import; copy it to the config agents dir to edit it`)
      return agentWrite(((await request.json()) as { source: string }).source)
    }),
    http.delete(`${B}/agents/:name`, ({ params }) => {
      const a = fx.agents.find((x) => x.name === params.name)
      if (a?.read_only) return apiError(422, -32602, `\`${a.name}\` is a read-only import; copy it to the config agents dir to remove it`)
      return HttpResponse.json(ok)
    }),

    http.get(`${B}/workflows`, () => HttpResponse.json(fx.workflows)),
    http.get(`${B}/workflows/runs`, ({ request }) => HttpResponse.json(paginate(fx.workflowRuns, new URL(request.url), 200))),
    http.get(`${B}/workflows/runs/:id`, ({ params }) => {
      const r = fx.workflowRuns.find((x) => x.id === params.id)
      return r ? HttpResponse.json({ ...fx.workflowRunDetail, ...r }) : apiError(404, -32003, `workflow run ${String(params.id)} not found`)
    }),
    http.post(`${B}/workflows/:name/run`, () => HttpResponse.json({ id: 'wf-new', session_id: 'sess-1' })),

    http.get(`${B}/config`, () => HttpResponse.json(fx.config)),
    http.patch(`${B}/config`, async ({ request }) => {
      const p = (await request.json()) as Record<string, Record<string, unknown>>
      const config: Record<string, unknown> = { ...fx.config.config }
      for (const [k, v] of Object.entries(p)) config[k] = { ...(config[k] as object), ...v }
      return HttpResponse.json({ ...fx.config, config, restart_required: 'ui' in p })
    }),
    http.post(`${B}/auth`, () => new HttpResponse(null, { status: 204 })),
  ]
}

/** Mirrors the server: the source needs frontmatter with a name and description. */
function agentWrite(source: string, status = 200) {
  if (!source.startsWith('---')) return apiError(422, -32602, 'the agent file must start with a `---` frontmatter block')
  if (!/^description:/m.test(source)) return apiError(422, -32602, 'the frontmatter needs a `description`')
  return HttpResponse.json({ ok: true, warnings: [] }, { status })
}

export const handlers = createHandlers()
