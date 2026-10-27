import { atom, read, update } from 'claude-code';
import type { EngineInterface, PluginState, Register, Timer } from 'claude-code';
import type { AttentionView, FetchState, LaneSegment, Snapshot, Tone } from '../types';
import { attentionCounts, newAttention, parseSnapshot, statusText, supervisorTone, themeInfo } from './model';
import { renderBand, renderPane } from './render';

const PANE = 'so-monitor';
const snapshot = atom({ plugin: 'superorchestrator', key: 'snapshot' } as const, null);
const fetch = atom({ plugin: 'superorchestrator', key: 'fetch' } as const, { status: 'idle', at: 0, error: null });
const orchestrator = atom({ plugin: 'superorchestrator', key: 'orchestrator' } as const, []);
const supervisor = atom({ plugin: 'superorchestrator', key: 'supervisor' } as const, []);
const theme = atom({ plugin: 'superorchestrator', key: 'theme' } as const, themeInfo(null));
const windowMin = atom({ plugin: 'superorchestrator', key: 'windowMin' } as const, 30);
const expanded = atom({ plugin: 'superorchestrator', key: 'expanded' } as const, []);
const clientPid = atom({ plugin: 'superorchestrator', key: 'clientPid' } as const, null);

async function readState($: EngineInterface): Promise<PluginState['superorchestrator']> {
  const values = await Promise.all([read($, snapshot), read($, fetch), read($, orchestrator),
    read($, supervisor), read($, theme), read($, windowMin), read($, expanded), read($, clientPid)]);
  return { snapshot: values[0], fetch: values[1], orchestrator: values[2], supervisor: values[3],
    theme: values[4], windowMin: values[5], expanded: values[6], clientPid: values[7] };
}

function trimLane(lane: LaneSegment[], now: number, minutes: number): LaneSegment[] {
  const cutoff = now - (minutes + 60) * 60_000;
  return lane.filter(s => s.to === null || s.to > cutoff)
    .map(s => ({ ...s, from: Math.max(cutoff, s.from) }));
}

function append(lane: LaneSegment[], tone: Tone, now: number, minutes: number): LaneSegment[] {
  const trimmed = trimLane(lane, now, minutes);
  const last = trimmed[trimmed.length - 1];
  if (last?.tone === tone && last.to === null) return trimmed;
  if (last?.to === null) last.to = now;
  return [...trimmed, { tone, from: now, to: null }];
}

type Runtime = { timer?: Timer; inFlight: boolean; stopped: boolean; lastPoll: number;
  cwd: string; previous: AttentionView | null; tools: Map<string, 'user' | 'waiting'>; idle: boolean };
function hasActiveRuns(data: Snapshot): boolean {
  return data.runs.some(run => !['succeeded', 'failed', 'cancelled'].includes(run.state));
}
function currentTone(runtime: Runtime): Tone {
  if ([...runtime.tools.values()].includes('user')) return 'user';
  if (runtime.tools.size > 0) return 'waiting';
  return runtime.idle ? 'idle' : 'claude';
}
function open($: EngineInterface, focus?: true) {
  return $.ui.open({ id: PANE, title: 'Superorchestrator', columns: 44, rows: 12, focus });
}
async function setOrchestratorTone($: EngineInterface, tone: Tone) {
  const now = await $.clock.now();
  const minutes = await read($, windowMin);
  await update($, orchestrator, list => append(list, tone, now, minutes));
}
async function readTheme($: EngineInterface) {
  const value = (await $.config.list()).find(row => row.key === 'theme')?.value;
  await update($, theme, () => themeInfo(typeof value === 'string' ? value : null));
}
// process.run documents both startup and timeout rejections without a stable
// discriminator. Only the shell's explicit command lookup proves absence.
async function checkBinary($: EngineInterface, runtime: Runtime): Promise<boolean> {
  const at = await $.clock.now();
  runtime.lastPoll = at;
  try {
    const result = await $.process.run(['sh', '-c', 'command -v superorchestrator'], { timeoutMs: 10000 });
    if (result.exitCode === 0) return true;
    if (result.exitCode === 1) {
      runtime.stopped = true;
      runtime.timer?.cancel();
      runtime.timer = undefined;
      await update($, fetch, (): FetchState => ({ status: 'missing', at, error: 'superorchestrator is not on PATH' }));
    } else {
      await update($, fetch, (): FetchState => ({ status: 'error', at,
        error: result.stderr || `PATH check failed (exit ${result.exitCode})` }));
    }
  } catch (error) {
    await update($, fetch, (): FetchState => ({ status: 'error', at,
      error: error instanceof Error ? error.message : String(error) }));
  }
  return false;
}
async function poll($: EngineInterface, runtime: Runtime) {
  if (runtime.inFlight || runtime.stopped) return;
  runtime.inFlight = true;
  try {
    const at = await $.clock.now();
    runtime.lastPoll = at;
    const minutes = await read($, windowMin);
    const pid = await read($, clientPid);
    let result;
    try {
      result = await $.process.run(['superorchestrator', 'monitor', '--json', '--client-pid',
        String(pid), '--cwd', runtime.cwd, '--window', String(minutes)], { timeoutMs: 10000 });
    } catch (error) {
      await update($, fetch, (): FetchState => ({ status: 'error', at,
        error: error instanceof Error ? error.message : String(error) }));
    }
    if (result) {
      const parsed = result.exitCode === 0 ? parseSnapshot(result.stdout) : null;
      if (parsed === null) {
        await update($, fetch, (): FetchState => ({ status: 'error', at, error: 'Monitor poll failed' }));
      } else {
        await update($, snapshot, () => parsed);
        await update($, fetch, (): FetchState => ({ status: parsed.supervisor.running ? 'ok' : 'down', at, error: null }));
        await update($, supervisor, list => append(list, supervisorTone(parsed.supervisor), at, minutes));
        const next = { snapshot: parsed, orchestrator: await read($, orchestrator) };
        for (const message of newAttention(runtime.previous, next)) $.ui.toast(message);
        runtime.previous = next;
      }
    }
    await update($, orchestrator, list => trimLane(list, at, minutes));
    await update($, supervisor, list => trimLane(list, at, minutes));
    const held = await readState($);
    const data = held.snapshot;
    $.ui.status(data ? statusText(attentionCounts(data, held.orchestrator, at),
      data.supervisor.running, hasActiveRuns(data), data.runs.length > 0) ?? undefined : undefined);
  } finally { runtime.inFlight = false; }
}
function startTimer($: EngineInterface, runtime: Runtime) {
  if (runtime.timer || runtime.stopped) return;
  runtime.timer = $.clock.every(2000, () => { void (async () => {
    const data = await read($, snapshot);
    const active = data?.supervisor.running && hasActiveRuns(data);
    if ((await $.clock.now()) - runtime.lastPoll >= (active ? 2000 : 10000)) await poll($, runtime);
  })(); });
}

export const register: Register = (on, options) => {
  const runtime: Runtime = { inFlight: false, stopped: false, lastPoll: -Infinity, cwd: '', previous: null, tools: new Map(), idle: false };
  on('session.start', async ($, e, next) => {
    runtime.cwd = e.cwd;
    const pid = await $.process.run(['sh', '-c', 'echo $PPID']);
    const value = Number(pid.stdout.trim());
    await update($, clientPid, () => Number.isInteger(value) && value > 0 ? value : null);
    await readTheme($);
    await $.command.register({ name: PANE, description: 'Open the superorchestrator monitor' });
    if (options.monitor !== false) await open($);
    if (await checkBinary($, runtime)) await poll($, runtime);
    startTimer($, runtime);
    return await next(e);
  });
  on('command.run', { command: PANE }, async $ => {
    await open($, true);
    runtime.stopped = false;
    if (await checkBinary($, runtime)) await poll($, runtime);
    startTimer($, runtime);
    return { text: 'Superorchestrator monitor opened.' };
  });
  on('turn.start', async ($, e, next) => {
    if (!e.agentId) { runtime.idle = false; await setOrchestratorTone($, currentTone(runtime)); }
    return await next(e);
  });
  on('turn.complete', async ($, e, next) => {
    if (!e.agentId) { runtime.idle = true; runtime.tools.clear(); await setOrchestratorTone($, 'idle'); }
    return await next(e);
  });
  on('tool.call', async ($, e, next) => {
    const tone = e.tool === 'AskUserQuestion' ? 'user'
      : e.tool === 'mcp__superorchestrator__wait_agent'
        || (e.tool === 'mcp__superorchestrator__spawn_agent' && e.wait === true) ? 'waiting' : null;
    if (tone === null || e.agentId) return await next(e);
    runtime.tools.set(e.tool_use_id, tone);
    await setOrchestratorTone($, currentTone(runtime));
    try { return await next(e); } finally {
      runtime.tools.delete(e.tool_use_id);
      await setOrchestratorTone($, currentTone(runtime));
    }
  });
  // tool.check reports a verdict, not the start/end of a permission dialog.
  // Its approver says who *could* answer; it cannot time an actual prompt.
  on('config.set', { key: 'theme' }, async ($, e, next) => {
    const result = await next(e);
    if (result.deny === undefined) await readTheme($);
    return result;
  });
  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) =>
    renderPane($.ui.resolve(e), await readState($), e.surface, e.props.bodyColumns,
      Math.min(e.viewport?.rows ?? e.props.scroll.bodyRows, e.props.scroll.bodyRows), await $.clock.now(), e.props.placement, {
        toggle: async id => { await update($, expanded, list => list.includes(id) ? list.filter(x => x !== id) : [...list, id]); },
        cycle: async () => {
          await update($, windowMin, value => value === 30 ? 60 : value === 60 ? 180 : 30);
          await poll($, runtime);
        },
      }));
  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if (options.band === false || e.props.hasSurvey) return next(e);
    return renderBand($.ui.resolve(e), await readState($), await $.clock.now()) ?? next(e);
  });
};
