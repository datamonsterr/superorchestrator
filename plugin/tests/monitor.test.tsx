import { expect, mock, test } from 'claude-code/testing';
import type { On } from 'claude-code';
import type { FetchState, LaneSegment } from '../types';
import { busySnapshot, emptySnapshot, downSnapshot } from './fixtures';

const PANE = {
  plugin: 'superorchestrator', component: 'Pane', requestId: 'so-monitor',
  props: { title: 'Superorchestrator', isFocused: false, bodyColumns: 40,
    placement: 'dock', scroll: { offset: 0, bodyRows: 30 }, view: {} },
} as const;
const START = { cwd: '/app', surface: 'terminal', isInteractive: true } as const;
function world(on: On, theme = 'dark', surfaces: ('terminal' | 'desktop')[] = ['terminal']) {
  const statuses: (string | undefined)[] = [];
  const toasts: string[] = [];
  const requests: { columns?: number; rows?: number; focus?: true }[] = [];
  let selectedTheme = theme;
  const clock = mock.clock(on, { now: Date.parse(busySnapshot().now) });
  on('command.register', () => ({ value: { command: 'so-monitor' } }));
  on('ui.open', (_, e) => { requests.push(e); return { value: { isPlaced: true } }; });
  on('config.list', () => ({ value: [{ key: 'theme', value: selectedTheme, label: 'Theme', kind: 'choice', provider: { plugin: 'engine', tier: 'core' }, isLocked: false }] }));
  on('session.surfaces', () => ({ value: surfaces }));
  on('session.start', (_, e) => ({ cwd: e.cwd }));
  on('ui.status', (_, e) => { statuses.push(e.text); return { value: undefined }; });
  on('ui.toast', (_, e) => { toasts.push(e.text); return { value: undefined }; });
  return { clock, statuses, toasts, requests, opens: () => requests.length, setTheme: (value: string) => { selectedTheme = value; } };
}
function fixture(on: On) {
  on('process.run', (_, e) => ({ value: { exitCode: 0,
    stdout: e.argv[0] === 'sh' ? '123\n' : JSON.stringify(busySnapshot()), stderr: '',
    isStdoutTruncated: false, isStderrTruncated: false } }));
}

test('pane shows quota, counts and every run; rows toggle their details on each surface', async ($, on) => {
  world(on); fixture(on);
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface });
    expect((await ui.find({ type: 'Text', text: /Superorchestrator/ }))?.text).toContain('Superorchestrator');
    expect((await ui.find({ type: 'Text', text: /quota/ }))?.text).toContain('week');
    expect((await ui.find({ type: 'Text', text: /1 Claude/ }))?.text).toContain('▶1 run');
    for (const run of busySnapshot().runs) expect(await ui.find({ key: run.id })).toBeDefined();
    await ui.press({ key: 'e3-core' });
    expect((await ui.find({ type: 'Text', text: /Ship the parser/ }))?.text).toContain('codex/gpt-6.1-sol/high');
    await ui.press({ key: 'e3-core' });
    expect(await ui.find({ type: 'Text', text: /Ship the parser/ })).toBeUndefined();
    await ui.unmount();
  }
});

const BAND = { plugin: 'superorchestrator', component: 'AbovePrompt',
  props: { hasSurvey: false, isWorking: false, maxRows: 5, bodyColumns: 40,
    scroll: { offset: 0, bodyRows: 5 }, view: {} } } as const;
const output = (stdout: string, exitCode = 0) => ({ value: { exitCode, stdout, stderr: '',
  isStdoutTruncated: false, isStderrTruncated: false } });

test('nested questions draw waiting lanes and do not notify Claude', async ($, on) => {
  const { clock, statuses, toasts } = world(on);
  const data = busySnapshot();
  const nested = data.runs[2]!;
  data.runs = [nested];
  nested.state = 'waiting';
  nested.segments = [{ state: 'waiting', from: '2026-10-25T14:29:35Z', to: null }];
  on('process.run', (_, e) => output(e.argv[0] === 'sh' ? '123' : JSON.stringify(data)));
  await $.session.start(START);
  nested.question = 'Which?';
  await clock.advance(2000);
  expect(statuses.at(-1)).toBe('SO');
  expect(toasts).toEqual([]);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface });
    expect((await ui.find({ key: nested.id }))?.text).not.toContain('◆');
    if (surface === 'terminal') {
      expect((await ui.find({ type: 'Text', text: /^░+$/ }))?.props.color).toBe('inactive');
    } else {
      expect((await ui.findAll({ type: 'Box' })).some(x => x.props.backgroundColor === 'claude')).toBe(false);
    }
    await ui.unmount();
  }
});

test('terminal lanes overlay marks and expanded details list mark times on every surface', async ($, on) => {
  world(on);
  const data = busySnapshot();
  const run = data.runs[2]!;
  data.runs = [run];
  run.marks = [{ ts: '2026-10-25T14:31:00Z', kind: 'failover' }];
  on('process.run', (_, e) => output(e.argv[0] === 'sh' ? '123' : JSON.stringify(data)));
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop', 'vscode'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface });
    if (surface === 'terminal') {
      expect((await ui.find({ key: run.id }))?.text).toContain('↯');
      expect((await ui.find({ type: 'Text', text: /^↯$/ }))?.props.color).toBe('warning');
    }
    await ui.press({ key: run.id });
    expect((await ui.find({ type: 'Text', text: /↯ failover/ }))?.text).toContain('14:31 ↯ failover');
    await ui.press({ key: run.id });
    await ui.unmount();
  }
});

test('polls with session arguments; status and new attention toasts follow snapshots', async ($, on) => {
  const { clock, statuses, toasts } = world(on);
  let polls = 0;
  on('process.run', (_, e) => {
    if (e.argv[0] === 'sh') return output('123\n');
    expect(e.argv).toEqual(['superorchestrator', 'monitor', '--json', '--client-pid', '123', '--cwd', '/app', '--window', '30']);
    expect(e.init?.timeoutMs).toBe(10000);
    const data = busySnapshot();
    if (polls++ > 0) { data.runs[2]!.attention = 'claude'; data.runs[2]!.state = 'pending_review'; }
    return output(JSON.stringify(data));
  });
  await $.session.start(START);
  expect(statuses).toEqual(['SO ▶1 ◆1 ⚠1 ✗1']);
  expect(toasts).toEqual([]);
  await clock.advance(1999); expect(polls).toBe(1);
  await clock.advance(1); expect(polls).toBe(2);
  expect(statuses[1]).toBe('SO ◆2 ⚠1 ✗1');
  expect(toasts).toEqual(['SO: fix-ui needs review']);
  await clock.advance(2000); expect(toasts).toHaveLength(1);
});

test('band shows attention and yields to the engine when nothing needs attention', async ($, on) => {
  const { clock } = world(on);
  let data = busySnapshot();
  on('process.run', (_, e) => output(e.argv[0] === 'sh' ? '123' : JSON.stringify(data)));
  on('ui.render', ($, e) => { const { Text } = $.ui.resolve(e); return <Text>engine band</Text>; });
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...BAND, surface });
    expect((await ui.find({ type: 'Text', text: /SO/ }))?.text).toContain('◆ 1 review');
    expect((await ui.find({ type: 'Text', text: /^✗/ }))?.props.color).toBe('error');
    await ui.unmount();
  }
  data = emptySnapshot();
  await clock.advance(2000);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...BAND, surface });
    expect((await ui.find({ type: 'Text' }))?.text).toBe('engine band');
    await ui.unmount();
  }
});

test('band option disables the attention band', { options: { band: false } }, async ($, on) => {
  world(on); fixture(on);
  on('ui.render', ($, e) => { const { Text } = $.ui.resolve(e); return <Text>engine band</Text>; });
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...BAND, surface });
    expect((await ui.find({ type: 'Text' }))?.text).toBe('engine band');
    await ui.unmount();
  }
});

test('absent binary stops polling; the command checks PATH again and restarts it', async ($, on) => {
  const { clock } = world(on);
  let missing = true;
  let checks = 0;
  let polls = 0;
  let fetchState: FetchState | undefined;
  on('state.set', async (_, e, next) => {
    if (e.key === 'fetch') fetchState = e.value;
    return next(e);
  });
  on('process.run', (_, e) => {
    if (e.argv[2] === 'command -v superorchestrator') {
      checks++; return output(missing ? '' : '/usr/bin/superorchestrator', missing ? 1 : 0);
    }
    if (e.argv[0] === 'sh') return output('123');
    polls++;
    return output(JSON.stringify(busySnapshot()));
  });
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' });
  expect((await ui.find({ type: 'Text', text: /not on PATH/ }))?.text).toBe('superorchestrator is not on PATH');
  expect(fetchState?.status).toBe('missing');
  await clock.advance(20000); expect(polls).toBe(0); expect(checks).toBe(1);
  missing = false;
  expect(await $.command.run({ command: 'so-monitor', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: false, columns: 80 } })).toEqual({ text: 'Superorchestrator monitor opened.' });
  expect(await ui.find({ key: 'e3-core' })).toBeDefined();
  await clock.advance(2000); expect(polls).toBe(2); expect(checks).toBe(2);
  await ui.unmount();
});

test('AskUserQuestion and wait tools change the orchestrator lane only during the call', async ($, on) => {
  const { clock } = world(on); fixture(on);
  on('turn.start', (_, e) => ({ turnId: e.turnId }));
  on('tool.call', async () => { await clock.sleep(1000); return { result: { ok: true } }; });
  let current: readonly LaneSegment[] = [];
  on('state.set', async (_, e, next) => {
    if (e.plugin === 'superorchestrator' && e.key === 'orchestrator') current = e.value as LaneSegment[];
    return next(e);
  });
  await $.session.start(START);
  await $.turn.start({ text: 'work', turnId: 'turn' });
  const question = $.tool.call({ tool: 'AskUserQuestion', questions: [] });
  await clock.settle();
  expect(current.at(-1)?.tone).toBe('user');
  await clock.advance(1000); await question;
  expect(current.at(-1)?.tone).toBe('claude');
  const wait = $.tool.call({ tool: 'mcp__superorchestrator__wait_agent', run_id: 'run' });
  await clock.settle();
  expect(current.at(-1)?.tone).toBe('waiting');
  await clock.advance(1000); await wait;
  expect(current.at(-1)?.tone).toBe('claude');
});

test('light theme changes empty track cells on both surfaces', async ($, on) => {
  world(on, 'light'); fixture(on);
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface });
    const empty = surface === 'terminal'
      ? (await ui.findAll({ type: 'Text' })).find(x => x.props.color === 'inactive' && x.props.dimColor === true && /■/.test(x.text))
      : (await ui.findAll({ type: 'Box' })).find(x => x.props.backgroundColor === 'inactive');
    expect(empty).toBeDefined();
    await ui.unmount();
  }
});

test('terminal inline layout packs quota cells across the width and keeps a timeline', async ($, on) => {
  world(on); fixture(on);
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal',
    props: { ...PANE.props, placement: 'inline', bodyColumns: 110 },
    viewport: { columns: 110, rows: 22, isFullscreen: false } });
  expect((await ui.find({ type: 'Text', text: /quota .*codex-acc2/ }))?.text).toContain('64%/51%');
  expect((await ui.find({ type: 'Text', text: /\+6 ok/ }))?.text).toContain('+6 ok');
  expect((await ui.find({ type: 'Text', text: /-30m/ }))?.text).toContain('now');
  expect(await ui.find({ key: 'e3-core' })).toBeDefined();
  await ui.unmount();
});

test('desktop dock layout stacks quota and timeline sections', async ($, on) => {
  world(on, 'dark', ['desktop']); fixture(on);
  await $.session.start({ ...START, surface: 'desktop' });
  const ui = await $.ui.mount({ ...PANE, surface: 'desktop' });
  expect((await ui.find({ type: 'Text', text: /^quota/ }))?.text).toBe('quota          5h        week');
  expect((await ui.find({ type: 'Text', text: /^codex-acc1/ }))?.text).toBe('codex-acc1 ');
  expect((await ui.find({ type: 'Text', text: /supervisor/ }))?.text).toContain('supervisor');
  await ui.unmount();
});

test('auto-opens with dimensions for either placement, and focuses on command', async ($, on) => {
  const { requests } = world(on); fixture(on);
  await $.session.start(START);
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({ columns: 44, rows: 12 });
  await $.command.run({ command: 'so-monitor', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: false, columns: 110 } });
  expect(requests[1]?.focus).toBe(true);
});

test('monitor false leaves auto-open disabled but the command opens it', { options: { monitor: false } }, async ($, on) => {
  const { opens } = world(on); fixture(on);
  await $.session.start(START);
  expect(opens()).toBe(0);
  await $.command.run({ command: 'so-monitor', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: false, columns: 80 } });
  expect(opens()).toBe(1);
});

test('idle or down sessions poll every ten seconds', async ($, on) => {
  const { clock } = world(on);
  let polls = 0;
  let data = emptySnapshot();
  on('process.run', (_, e) => {
    if (e.argv[0] === 'sh') return output('123');
    polls++; return output(JSON.stringify(data));
  });
  await $.session.start(START);
  await clock.advance(9999); expect(polls).toBe(1);
  data = downSnapshot();
  await clock.advance(1); expect(polls).toBe(2);
  await clock.advance(9999); expect(polls).toBe(2);
  await clock.advance(1); expect(polls).toBe(3);
});

test('poll errors retain the last good rows', async ($, on) => {
  const { clock } = world(on);
  let polls = 0;
  on('process.run', (_, e) => {
    if (e.argv[0] === 'sh') return output('123');
    polls++;
    return polls === 1 ? output(JSON.stringify(busySnapshot())) : polls === 2 ? output('{invalid') : output('', 1);
  });
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' });
  for (let i = 0; i < 2; i++) {
    await clock.advance(2000);
    expect((await ui.find({ type: 'Text', text: /Monitor poll failed/ }))?.text).toContain('last good data');
    expect(await ui.find({ key: 'e3-core' })).toBeDefined();
  }
  await ui.unmount();
});

test('only one monitor process runs at once', async ($, on) => {
  const { clock } = world(on);
  let polls = 0;
  on('process.run', async (_, e) => {
    if (e.argv[0] === 'sh') return output('123');
    polls++;
    if (polls > 1) await clock.sleep(5000);
    return output(JSON.stringify(busySnapshot()));
  });
  await $.session.start(START);
  await clock.advance(2000); expect(polls).toBe(2);
  await clock.advance(4000); expect(polls).toBe(2);
  await clock.advance(1000); expect(polls).toBe(2);
});

test('theme changes pass through and redraw empty track colors', async ($, on) => {
  const { clock, setTheme } = world(on, 'light'); fixture(on);
  on('config.set', (_, e) => { setTheme(String(e.value)); return { value: e.value }; });
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' });
  const empty = async () => (await ui.findAll({ type: 'Text' })).find(x => x.props.dimColor === true && /■/.test(x.text));
  expect((await empty())?.props.color).toBe('inactive');
  expect(await $.config.set({ key: 'theme', value: 'dark', previous: 'light', origin: { kind: 'composer' }, provider: { plugin: 'engine', tier: 'core' } })).toEqual({ value: 'dark' });
  await clock.settle();
  expect((await ui.findAll({ type: 'Text' })).some(x => x.props.color === 'subtle' && /■/.test(x.text))).toBe(true);
  await ui.unmount();
});

test('window button cycles 30, 60 and 180 minutes and repolls', async ($, on) => {
  world(on);
  const windows: string[] = [];
  on('process.run', (_, e) => {
    if (e.argv[0] === 'sh') return output('123');
    windows.push(e.argv.at(-1)!); return output(JSON.stringify(busySnapshot()));
  });
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' });
  for (let i = 0; i < 3; i++) await ui.press({ key: 'window' });
  expect(windows).toEqual(['30', '60', '180', '30']);
  expect((await ui.find({ key: 'window' }))?.props.hotkey).toBe('w');
  await ui.unmount();
});

test('short pane preserves attention rows and reports hidden runs', async ($, on) => {
  world(on); fixture(on);
  await $.session.start(START);
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface, viewport: { columns: 44, rows: 12, isFullscreen: true } });
    expect(await ui.find({ key: 'e3-core' })).toBeDefined();
    expect(await ui.find({ key: 'e4-sales' })).toBeDefined();
    expect(await ui.find({ key: 'fix-ui' })).toBeUndefined();
    expect((await ui.find({ type: 'Text', text: /more$/ }))?.text).toBe('+3 more');
    await ui.unmount();
  }
});

test('vscode uses bars and mobile shows run marks without bars', async ($, on) => {
  world(on); fixture(on);
  await $.session.start(START);
  for (const surface of ['vscode', 'mobile'] as const) {
    const ui = await $.ui.mount({ ...PANE, surface });
    expect(await ui.find({ key: 'e3-core' })).toBeDefined();
    expect((await ui.findAll({ type: 'Box' })).some(x => x.props.backgroundColor === 'claude')).toBe(surface === 'vscode');
    if (surface === 'mobile') expect((await ui.findAll({ type: 'Box' })).filter(x => x.props.backgroundColor)).toHaveLength(0);
    await ui.unmount();
  }
});

test('a concurrent wait finishing keeps the person lane until their question settles', async ($, on) => {
  const { clock } = world(on); fixture(on);
  let current: readonly LaneSegment[] = [];
  on('state.set', async (_, e, next) => {
    if (e.plugin === 'superorchestrator' && e.key === 'orchestrator') current = e.value;
    return next(e);
  });
  on('tool.call', async (_, e) => {
    await clock.sleep(e.tool === 'AskUserQuestion' ? 2000 : 1000);
    return { result: { tool: e.tool } };
  });
  await $.session.start(START);
  const wait = $.tool.call({ tool: 'mcp__superorchestrator__wait_agent', run_id: 'run' });
  await clock.settle();
  const question = $.tool.call({ tool: 'AskUserQuestion', questions: [] });
  await clock.settle();
  await clock.advance(1000); await wait;
  const whileQuestion = current.at(-1)?.tone;
  await clock.advance(1000);
  expect(await question).toMatchObject({ result: { tool: 'AskUserQuestion' } });
  expect(whileQuestion).toBe('user');
  expect(current.at(-1)?.tone).toBe('claude');
});

test('supervisor samples merge, and completed turns open idle without observing subagent turns', async ($, on) => {
  const { clock } = world(on); fixture(on);
  let supervisorLane: readonly LaneSegment[] = [];
  let claudeLane: readonly LaneSegment[] = [];
  on('state.set', async (_, e, next) => {
    if (e.key === 'supervisor') supervisorLane = e.value;
    if (e.key === 'orchestrator') claudeLane = e.value;
    return next(e);
  });
  on('turn.start', (_, e) => ({ turnId: e.turnId }));
  on('turn.complete', (_, e) => ({ text: e.answer }));
  await $.session.start(START);
  await clock.advance(2000);
  expect(supervisorLane).toHaveLength(1);
  await $.turn.start({ text: 'work', turnId: 'main' });
  await clock.advance(1000);
  expect(await $.turn.complete({ answer: 'done', reason: 'answer', durationMs: 1000, isAborted: false, turnId: 'main' })).toEqual({ text: 'done' });
  expect(claudeLane.map(s => s.tone)).toEqual(['claude', 'idle']);
  await $.turn.start({ text: 'subagent', turnId: 'child', agentId: 'child' });
  expect(claudeLane.at(-1)?.tone).toBe('idle');
});

test('desktop renders fractional quota percentages as supported bar widths', async ($, on) => {
  world(on);
  const data = busySnapshot();
  data.accounts[0]!.pools[0]!.five_hour_pct = 64.5;
  on('process.run', (_, e) => output(e.argv[0] === 'sh' ? '123' : JSON.stringify(data)));
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'desktop' });
  expect(await ui.find({ key: 'e3-core' })).toBeDefined();
  await ui.unmount();
});

test('terminal run states use the idle poll cadence even without a finish timestamp', async ($, on) => {
  const { clock } = world(on);
  const data = busySnapshot();
  data.runs = data.runs.filter(run => run.state === 'succeeded' || run.state === 'failed');
  for (const run of data.runs) run.finished_at = null;
  let polls = 0;
  on('process.run', (_, e) => {
    if (e.argv[0] === 'sh') return output('123');
    polls++; return output(JSON.stringify(data));
  });
  await $.session.start(START);
  await clock.advance(9999); expect(polls).toBe(1);
  await clock.advance(1); expect(polls).toBe(2);
});

test('timeout and transient process rejections preserve data and keep polling', async ($, on) => {
  const { clock } = world(on);
  let polls = 0;
  let checks = 0;
  let fetchState: FetchState | undefined;
  on('state.set', async (_, e, next) => {
    if (e.key === 'fetch') fetchState = e.value;
    return next(e);
  });
  on('process.run', (_, e) => {
    if (e.argv[2] === 'command -v superorchestrator') { checks++; return output('/usr/bin/superorchestrator'); }
    if (e.argv[0] === 'sh') return output('123');
    polls++;
    if (polls === 2) return { deny: 'process.run timed out after 10000ms' };
    if (polls === 3) return { deny: 'temporary process transport failure' };
    return output(JSON.stringify(busySnapshot()));
  });
  await $.session.start(START);
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' });
  await clock.advance(2000);
  expect(fetchState?.status).toBe('error');
  expect(fetchState?.error).toContain('timed out after 10000ms');
  expect(await ui.find({ key: 'e3-core' })).toBeDefined();
  await clock.advance(2000);
  expect(polls).toBe(3);
  expect(fetchState?.status).toBe('error');
  expect(fetchState?.error).toContain('temporary process transport failure');
  await clock.advance(2000);
  expect(polls).toBe(4);
  expect(fetchState?.status).toBe('ok');
  expect(checks).toBe(1);
  await ui.unmount();
});
