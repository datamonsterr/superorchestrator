/// <reference types="node" />
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import fs from 'node:fs'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'

/**
 * Real-server e2e fixture: builds web/dist and the binaries, starts a supervisor on a temporary config dir, socket and
 * HOME, fills it through `e2e-seed` (fake-harness accounts, no real harness ever runs) and starts `superorchestrator ui`
 * on a free port. The URL and ids go to the specs through environment variables; `teardown` undoes it all.
 */
const webDir = path.resolve(import.meta.dirname, '../..')
const repo = path.resolve(webDir, '..')
const STATE = path.join(os.tmpdir(), 'so-e2e-real-state.json')

interface State {
  tmp: string
  seedPid: number
  daemonPid: number | null
}

function sh(cmd: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv) {
  execFileSync(cmd, args, { cwd, stdio: 'inherit', env: env ?? process.env })
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer()
    s.once('error', reject)
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address() as net.AddressInfo
      s.close(() => resolve(port))
    })
  })
}

function readyLine(child: ChildProcess, ms: number): Promise<{ runs: Record<string, string> }> {
  return new Promise((resolve, reject) => {
    let buf = ''
    const timer = setTimeout(() => reject(new Error(`e2e-seed was not ready in ${ms} ms:\n${buf}`)), ms)
    child.stdout!.on('data', (d: Buffer) => {
      buf += d.toString()
      const line = buf.split('\n').find((l) => l.startsWith('{'))
      if (line) {
        clearTimeout(timer)
        resolve(JSON.parse(line))
      }
    })
    child.once('exit', (code) => reject(new Error(`e2e-seed exited with ${code}:\n${buf}`)))
  })
}

export default async function globalSetup() {
  // 1. The UI first: the binary embeds web/dist at compile time.
  sh('pnpm', ['build'], webDir)
  // 2. The binaries (debug is enough): the CLI and the seeder, and the scripted stand-in for a harness.
  sh('cargo', ['build', '--locked', '-p', 'superorchestrator', '--features', 'test-support', '--bin', 'superorchestrator', '--bin', 'e2e-seed'], repo)
  sh('cargo', ['build', '--locked', '-p', 'so-harness', '--features', 'test-support', '--bin', 'fake-harness'], repo)
  const meta = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], { cwd: repo, maxBuffer: 1 << 28 }).toString())
  const bin = path.join(meta.target_directory, 'debug')

  // 3. A private world. Unix socket paths are short (SUN_LEN), so stay under the system temp dir.
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'so-e2e-'))
  const dirs = { home: path.join(tmp, 'home'), cfg: path.join(tmp, 'cfg'), run: path.join(tmp, 'run'), cwd: path.join(tmp, 'work') }
  for (const d of Object.values(dirs)) fs.mkdirSync(d, { recursive: true, mode: 0o700 })
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    HOME: dirs.home,
    SUPERORCHESTRATOR_CONFIG_DIR: dirs.cfg,
    SUPERORCHESTRATOR_SOCKET: path.join(dirs.run, 'so.sock'),
    XDG_CONFIG_HOME: path.join(dirs.home, '.config'),
  }
  delete env.SUPERORCHESTRATOR_LOG
  const so = path.join(bin, 'superorchestrator')
  sh(so, ['init'], repo, env)

  // 4. Seed: accounts on the fake harness, and one run per state worth drawing.
  const seed = spawn(
    path.join(bin, 'e2e-seed'),
    ['--config-dir', dirs.cfg, '--home', dirs.home, '--cwd', dirs.cwd, '--fake-harness', path.join(bin, 'fake-harness'), '--daemon-exe', so],
    { env, stdio: ['ignore', 'pipe', 'inherit'] },
  )
  const seeded = await readyLine(seed, 90_000)

  // 5. The UI, on a free port.
  const port = await freePort()
  const ui = JSON.parse(execFileSync(so, ['--json', 'ui', '--no-open', '--port', String(port)], { env }).toString())
  const status = JSON.parse(execFileSync(so, ['--json', 'status'], { env }).toString())
  const loginUrl = new URL(ui.url)
  const code = new URLSearchParams(loginUrl.hash.slice(1)).get('token')
  if (!code) throw new Error('UI sign-in URL did not carry a login code')
  const login = await fetch(`${loginUrl.origin}/api/v1/token`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token: code }),
  })
  if (!login.ok) throw new Error(`Code exchange failed: ${login.status}`)
  const cookie = login.headers.get('set-cookie')?.split(';')[0]
  if (!cookie) throw new Error('Code exchange did not set a cookie')
  const sessionsResponse = await fetch(`${loginUrl.origin}/api/v1/sessions`, { headers: { Cookie: cookie } })
  if (!sessionsResponse.ok) throw new Error(`Session lookup failed: ${sessionsResponse.status}`)
  const sessions = await sessionsResponse.json()

  const state: State = { tmp, seedPid: seed.pid!, daemonPid: status.supervisor?.pid ?? null }
  fs.writeFileSync(STATE, JSON.stringify(state))
  process.env.E2E_REAL_URL = loginUrl.origin
  process.env.E2E_REAL_CLI = so
  process.env.E2E_REAL_CONFIG_DIR = dirs.cfg
  process.env.E2E_REAL_HOME = dirs.home
  process.env.E2E_REAL_SOCKET = env.SUPERORCHESTRATOR_SOCKET
  const idOf = (client: string) => sessions.items.find((x: { client: string }) => x.client === client).id
  process.env.E2E_REAL_SESSION = idOf('e2e-seed')
  process.env.E2E_REAL_EMPTY_SESSION = idOf('e2e-empty')
  process.env.E2E_REAL_RUNS = JSON.stringify(seeded.runs)
  return teardown
}

async function teardown() {
  if (!fs.existsSync(STATE)) return
  const s: State = JSON.parse(fs.readFileSync(STATE, 'utf8'))
  for (const pid of [s.seedPid, s.daemonPid]) {
    if (!pid) continue
    try {
      process.kill(pid)
    } catch {
      // already gone
    }
  }
  fs.rmSync(s.tmp, { recursive: true, force: true })
  fs.rmSync(STATE, { force: true })
}
