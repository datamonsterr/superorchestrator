import { describe, expect, test } from 'claude-code/testing';

import type {
  AttentionCounts,
  LaneSegment,
  Pool,
  QuotaCell,
  QuotaRow,
  Run,
  Snapshot,
  Tone,
} from '../types';
import {
  TONES,
  attentionCounts,
  axisLabels,
  bandText,
  barCells,
  formatDuration,
  labelWidth,
  laneRuns,
  laneSpans,
  newAttention,
  parseSnapshot,
  quotaRows,
  runLabel,
  runRows,
  statusText,
  supervisorTone,
  themeInfo,
  toneOfRunState,
  trackColor,
} from '../hooks/model';
import { busySnapshot, downSnapshot, emptySnapshot } from './fixtures';

const at = (iso: string): number => Date.parse(iso);
const lane = (tone: Tone, from: number, to: number | null): LaneSegment => ({
  tone,
  from,
  to,
});

function json(snapshot: Snapshot): Record<string, unknown> {
  return JSON.parse(JSON.stringify(snapshot)) as Record<string, unknown>;
}

function firstRun(raw: Record<string, unknown>): Record<string, unknown> {
  const runs = raw['runs'] as Record<string, unknown>[];
  const run = runs[0];
  if (run === undefined) throw new Error('fixture has no runs');
  return run;
}

function firstPool(raw: Record<string, unknown>): Record<string, unknown> {
  const accounts = raw['accounts'] as Record<string, unknown>[];
  const pools = accounts[0]?.['pools'] as Record<string, unknown>[];
  const pool = pools[0];
  if (pool === undefined) throw new Error('fixture has no pools');
  return pool;
}

function zeroCounts(): AttentionCounts {
  return { running: 0, claude: 0, user: 0, stuck: 0, failed: 0 };
}

function emptyRow(label: string): QuotaRow {
  return {
    label,
    kind: 'pool',
    fiveHour: { pct: null, text: '—', tone: 'inactive' },
    week: { pct: null, text: '—', tone: 'inactive' },
    note: null,
    noteTone: null,
  };
}

function quotaTestCell(pct: number | null): QuotaCell {
  if (pct === null) return { pct, text: '—', tone: 'inactive' };
  return {
    pct,
    text: `${pct}%`,
    tone: pct >= 50 ? 'success' : pct >= 15 ? 'warning' : 'error',
  };
}

function poolRow(label: string, five: number | null, week: number | null): QuotaRow {
  return {
    label,
    kind: 'pool',
    fiveHour: quotaTestCell(five),
    week: quotaTestCell(week),
    note: null,
    noteTone: null,
  };
}

function accountWith(pools: Pool[]): Snapshot['accounts'][number] {
  return {
    name: 'acc',
    harness: 'codex',
    enabled: true,
    signed_in: 'yes',
    active_runs: 0,
    cap: 2,
    pools,
  };
}

function poolWith(over: Partial<Pool>): Pool {
  return {
    pool: 'default',
    status: 'ok',
    five_hour_pct: 80,
    five_hour_resets_at: null,
    weekly_pct: 80,
    weekly_resets_at: null,
    cooldown_until: null,
    source: 'probe',
    observed_at: null,
    ...over,
  };
}

function runWith(over: Partial<Run>): Run {
  const base: Run = {
    id: 'r',
    name: 'r',
    parent_run: null,
    depth: 1,
    harness: 'codex',
    account: null,
    model: null,
    state: 'running',
    attention: null,
    question: null,
    created_at: '2026-10-25T14:00:00Z',
    started_at: '2026-10-25T14:00:00Z',
    finished_at: null,
    segments: [],
    marks: [],
  };
  return { ...base, ...over };
}

// ---------------------------------------------------------------------------

describe('parseSnapshot', () => {
  test('reads the busy fixture unchanged', () => {
    const busy = busySnapshot();
    expect(parseSnapshot(JSON.stringify(busy))).toEqual(busy);
  });

  test('accepts a null session and empty lists', () => {
    const empty = emptySnapshot();
    expect(parseSnapshot(JSON.stringify(empty))).toEqual(empty);
    const down = downSnapshot();
    expect(parseSnapshot(JSON.stringify(down))).toEqual(down);
  });

  test('accepts a null run name, session cwd and pool source', () => {
    const down = downSnapshot();
    const parsed = parseSnapshot(JSON.stringify(down));
    expect(parsed).not.toBeNull();
    expect(parsed?.runs[0]?.name).toBeNull();
    expect(parsed?.session?.cwd).toBeNull();
    expect(parsed?.accounts[0]?.pools[0]?.source).toBeNull();
  });

  test('accepts a fractional headroom percent', () => {
    const raw = json(busySnapshot());
    firstPool(raw)['five_hour_pct'] = 59.5;
    const parsed = parseSnapshot(JSON.stringify(raw));
    expect(parsed).not.toBeNull();
    const row = quotaRows(parsed?.accounts ?? [], 20).find(
      (candidate) => candidate.label === 'codex-acc1',
    );
    expect(row?.fiveHour).toEqual({ pct: 59.5, text: '60%', tone: 'success' });
  });

  test('returns null for anything that is not JSON', () => {
    expect(parseSnapshot('')).toBeNull();
    expect(parseSnapshot('{')).toBeNull();
    expect(parseSnapshot('not json')).toBeNull();
  });

  test('returns null for JSON that is not a snapshot', () => {
    expect(parseSnapshot('null')).toBeNull();
    expect(parseSnapshot('[]')).toBeNull();
    expect(parseSnapshot('{}')).toBeNull();
    expect(parseSnapshot('42')).toBeNull();
  });

  test('returns null when a required field is missing', () => {
    const raw = json(busySnapshot());
    delete firstRun(raw)['state'];
    expect(parseSnapshot(JSON.stringify(raw))).toBeNull();
  });

  test('returns null when a state is unknown', () => {
    const raw = json(busySnapshot());
    firstRun(raw)['state'] = 'sleeping';
    expect(parseSnapshot(JSON.stringify(raw))).toBeNull();
  });

  test('returns null when a timestamp does not parse', () => {
    const raw = json(busySnapshot());
    firstRun(raw)['created_at'] = 'yesterday';
    expect(parseSnapshot(JSON.stringify(raw))).toBeNull();
  });

  test('returns null when a percent is out of range', () => {
    const raw = json(busySnapshot());
    firstPool(raw)['five_hour_pct'] = 140;
    expect(parseSnapshot(JSON.stringify(raw))).toBeNull();
  });

  test('returns null when the supervisor has the wrong type', () => {
    const raw = json(busySnapshot());
    (raw['supervisor'] as Record<string, unknown>)['running'] = 'yes';
    expect(parseSnapshot(JSON.stringify(raw))).toBeNull();
  });

  test('ignores fields it does not know', () => {
    const busy = busySnapshot();
    const raw = json(busy);
    raw['future'] = { added: true };
    const parsed = parseSnapshot(JSON.stringify(raw));
    expect(parsed).not.toBeNull();
    expect(parsed?.now).toBe(busy.now);
    expect(parsed?.runs.map((run) => run.id)).toEqual(busy.runs.map((run) => run.id));
  });
});

describe('tones', () => {
  test('maps every run state', () => {
    expect(toneOfRunState('queued', false)).toBe('queued');
    expect(toneOfRunState('starting', false)).toBe('running');
    expect(toneOfRunState('running', false)).toBe('running');
    expect(toneOfRunState('waiting', false)).toBe('waiting');
    expect(toneOfRunState('stalled', false)).toBe('stuck');
    expect(toneOfRunState('escalated', false)).toBe('stuck');
    expect(toneOfRunState('pending_review', false)).toBe('review');
    expect(toneOfRunState('succeeded', false)).toBe('done');
    expect(toneOfRunState('failed', false)).toBe('failed');
    expect(toneOfRunState('cancelled', false)).toBe('cancelled');
  });

  test('a question for the orchestrator makes a live run review', () => {
    expect(toneOfRunState('waiting', true)).toBe('review');
    expect(toneOfRunState('running', true)).toBe('review');
    expect(toneOfRunState('pending_review', true)).toBe('review');
  });

  test('a question never overwrites a terminal or stuck tone', () => {
    expect(toneOfRunState('failed', true)).toBe('failed');
    expect(toneOfRunState('succeeded', true)).toBe('done');
    expect(toneOfRunState('cancelled', true)).toBe('cancelled');
    expect(toneOfRunState('stalled', true)).toBe('stuck');
  });

  test('the tone table follows the theme, bar and mark', () => {
    expect(TONES.running).toEqual({ theme: 'ide', bar: '█', mark: '▶', dim: false });
    expect(TONES.claude).toEqual({ theme: 'claude', bar: '█', mark: '◆', dim: false });
    expect(TONES.review).toEqual({ theme: 'claude', bar: '▓', mark: '◆', dim: false });
    expect(TONES.user).toEqual({ theme: 'bashBorder', bar: '█', mark: '●', dim: false });
    expect(TONES.stuck).toEqual({ theme: 'warning', bar: '▒', mark: '⚠', dim: false });
    expect(TONES.failed).toEqual({ theme: 'error', bar: null, mark: '✗', dim: false });
    expect(TONES.done).toEqual({ theme: 'success', bar: null, mark: '✓', dim: false });
    expect(TONES.waiting).toEqual({ theme: 'inactive', bar: '░', mark: null, dim: false });
    expect(TONES.queued).toEqual({ theme: 'inactive', bar: '·', mark: '○', dim: true });
    expect(TONES.idle).toEqual({ theme: 'subtle', bar: '░', mark: null, dim: false });
    expect(TONES.cancelled).toEqual({ theme: 'inactive', bar: null, mark: '■', dim: false });
    expect(TONES.up).toEqual({ theme: 'success', bar: '█', mark: null, dim: false });
    expect(TONES.stale).toEqual({ theme: 'warning', bar: '▒', mark: null, dim: false });
    expect(TONES.down).toEqual({ theme: 'error', bar: '·', mark: null, dim: false });
  });

  test('reads the supervisor lane tone from its health', () => {
    expect(supervisorTone(busySnapshot().supervisor)).toBe('up');
    expect(supervisorTone(downSnapshot().supervisor)).toBe('down');
    const stale = busySnapshot().supervisor;
    stale.stale = true;
    expect(supervisorTone(stale)).toBe('stale');
  });
});

test('a supervisor warning uses the stale tone even without the stale flag', () => {
  const health = busySnapshot().supervisor;
  health.warning = 'build differs from the running supervisor';
  expect(supervisorTone(health)).toBe('stale');
});

describe('theme', () => {
  test('reads the six named themes', () => {
    expect(themeInfo('dark')).toEqual({ mode: 'dark', ansi: false, daltonized: false });
    expect(themeInfo('light')).toEqual({ mode: 'light', ansi: false, daltonized: false });
    expect(themeInfo('dark-daltonized')).toEqual({ mode: 'dark', ansi: false, daltonized: true });
    expect(themeInfo('light-daltonized')).toEqual({ mode: 'light', ansi: false, daltonized: true });
    expect(themeInfo('dark-ansi')).toEqual({ mode: 'dark', ansi: true, daltonized: false });
    expect(themeInfo('light-ansi')).toEqual({ mode: 'light', ansi: true, daltonized: false });
  });

  test('anything else is dark', () => {
    expect(themeInfo('solarized')).toEqual({ mode: 'dark', ansi: false, daltonized: false });
    expect(themeInfo(null)).toEqual({ mode: 'dark', ansi: false, daltonized: false });
    expect(themeInfo(undefined)).toEqual({ mode: 'dark', ansi: false, daltonized: false });
  });

  test('empty tracks are subtle on dark and dim inactive on light', () => {
    expect(trackColor(themeInfo('dark'))).toEqual({ color: 'subtle', dim: false });
    expect(trackColor(themeInfo('dark-daltonized'))).toEqual({ color: 'subtle', dim: false });
    expect(trackColor(themeInfo('light'))).toEqual({ color: 'inactive', dim: true });
    expect(trackColor(themeInfo('light-ansi'))).toEqual({ color: 'inactive', dim: true });
  });
});

describe('attentionCounts', () => {
  test('only root questions need the orchestrator', () => {
    const snapshot = emptySnapshot();
    const nested = runWith({ state: 'waiting', depth: 2, parent_run: 'parent', question: 'Which?' });
    snapshot.runs = [nested];
    expect(attentionCounts(snapshot, [], at(snapshot.now))).toEqual(zeroCounts());
    snapshot.runs = [runWith({ state: 'waiting', question: 'Which?' })];
    expect(attentionCounts(snapshot, [], at(snapshot.now)).claude).toBe(1);
  });

  test('counts the busy fixture', () => {
    const counts = attentionCounts(busySnapshot(), [], at('2026-10-25T14:32:05Z'));
    expect(counts).toEqual({ running: 1, claude: 1, user: 0, stuck: 1, failed: 1 });
  });

  test('a question on a running run counts as claude, not running', () => {
    const snapshot = emptySnapshot();
    snapshot.runs = [
      runWith({ id: 'q', state: 'running', question: 'Which?' }),
      runWith({ id: 'r', state: 'running' }),
    ];
    const counts = attentionCounts(snapshot, [], at('2026-10-25T14:32:05Z'));
    expect(counts.running).toBe(1);
    expect(counts.claude).toBe(1);
  });

  test('attention drives a count even when the state tone would not', () => {
    const snapshot = emptySnapshot();
    snapshot.runs = [
      runWith({ id: 'a', state: 'waiting', attention: 'claude' }),
      runWith({ id: 'b', state: 'running', attention: 'stuck' }),
    ];
    const counts = attentionCounts(snapshot, [], at('2026-10-25T14:32:05Z'));
    expect(counts).toEqual({ running: 0, claude: 1, user: 0, stuck: 1, failed: 0 });
  });

  test('user comes from the orchestrator lane open at now', () => {
    const snapshot = emptySnapshot();
    const now = at('2026-10-25T14:32:05Z');
    expect(
      attentionCounts(snapshot, [lane('user', now - 1000, null)], now).user,
    ).toBe(1);
    expect(
      attentionCounts(snapshot, [lane('user', now - 2000, now - 1000)], now).user,
    ).toBe(0);
    expect(
      attentionCounts(snapshot, [lane('user', now + 1000, null)], now).user,
    ).toBe(0);
    expect(attentionCounts(snapshot, [lane('claude', now - 1000, null)], now).user).toBe(0);
  });
});

describe('quotaRows', () => {
  test('sorts worst first when everything fits', () => {
    const rows = quotaRows(busySnapshot().accounts, 20);
    expect(rows.map((row) => row.label)).toEqual([
      'codex-acc2',
      'agy-acc1/g',
      'codex-acc1',
      'ok5',
      'ok4',
      'ok3',
      'ok2',
      'ok1',
      'agy-acc1/d',
    ]);
    expect(rows[0]?.kind).toBe('cooldown');
    expect(rows[0]?.note).toMatch(/^cooldown until \d\d:\d\d$/);
    expect(rows[0]?.noteTone).toBe('error');
    expect(rows[1]?.kind).toBe('pool');
    expect(rows[1]?.fiveHour).toEqual({ pct: 12, text: '12%', tone: 'error' });
    expect(rows[1]?.week).toEqual({ pct: 80, text: '80%', tone: 'success' });
    expect(rows[1]?.note).toBe('low');
    expect(rows[1]?.noteTone).toBe('warning');
  });

  test('folds the healthiest pools when maxRows is short', () => {
    const rows = quotaRows(busySnapshot().accounts, 4);
    expect(rows.map((row) => row.label)).toEqual([
      'codex-acc2',
      'agy-acc1/g',
      'codex-acc1',
      '+6 ok ≥50%',
    ]);
    expect(rows[3]?.kind).toBe('folded');
    expect(rows[3]?.noteTone).toBe('success');
  });

  test('maxRows 1 keeps the worst row', () => {
    const rows = quotaRows(busySnapshot().accounts, 1);
    expect(rows.map((row) => row.label)).toEqual(['codex-acc2']);
  });

  test('maxRows 0 shows nothing', () => {
    expect(quotaRows(busySnapshot().accounts, 0)).toEqual([]);
  });

  test('empty accounts show nothing', () => {
    expect(quotaRows([], 10)).toEqual([]);
  });

  test('all healthy pools fold to one line when there is no room', () => {
    const accounts = busySnapshot().accounts.filter((account) =>
      ['ok1', 'ok2', 'ok3', 'ok4', 'ok5'].includes(account.name),
    );
    const rows = quotaRows(accounts, 1);
    expect(rows.map((row) => row.label)).toEqual(['+5 ok ≥50%']);
  });

  test('keeps the fold line last after an unknown pool', () => {
    const unknown = {
      ...accountWith([
        poolWith({ status: 'unknown', five_hour_pct: null, weekly_pct: null }),
      ]),
      name: 'zz-unknown',
    };
    const accounts = [
      unknown,
      ...busySnapshot().accounts.filter((account) =>
        ['ok1', 'ok2', 'ok3'].includes(account.name),
      ),
    ];
    const rows = quotaRows(accounts, 3);
    expect(rows.map((row) => row.label)).toEqual(['ok3', 'zz-unknown', '+2 ok ≥50%']);
    expect(rows[2]?.kind).toBe('folded');
  });

  test('uses 50/15 thresholds', () => {
    const pools = [
      poolWith({ pool: 'a', five_hour_pct: 50, weekly_pct: 50 }),
      poolWith({ pool: 'b', five_hour_pct: 49, weekly_pct: 15 }),
      poolWith({ pool: 'c', five_hour_pct: 14, weekly_pct: 0 }),
    ];
    const rows = quotaRows([accountWith(pools)], 10);
    expect(rows.map((row) => row.label)).toEqual(['acc/c', 'acc/b', 'acc/a']);
    expect(rows[0]?.fiveHour.tone).toBe('error');
    expect(rows[1]?.fiveHour.tone).toBe('warning');
    expect(rows[1]?.week.tone).toBe('warning');
    expect(rows[2]?.fiveHour.tone).toBe('success');
  });

  test('an exhausted pool without a cooldown says exhausted in error', () => {
    const rows = quotaRows(
      [accountWith([poolWith({ status: 'exhausted', five_hour_pct: 0, weekly_pct: 0 })])],
      10,
    );
    expect(rows[0]?.kind).toBe('exhausted');
    expect(rows[0]?.note).toBe('exhausted');
    expect(rows[0]?.noteTone).toBe('error');
  });

  test('a ledger pool with no percentages says ledger', () => {
    const rows = quotaRows(
      [
        accountWith([
          poolWith({ source: 'ledger', five_hour_pct: null, weekly_pct: null }),
        ]),
      ],
      10,
    );
    expect(rows[0]?.kind).toBe('ledger');
    expect(rows[0]?.note).toBe('ledger');
    expect(rows[0]?.noteTone).toBe('inactive');
  });

  test('an unknown pool shows an inactive dash', () => {
    const rows = quotaRows(
      [
        accountWith([
          poolWith({ status: 'unknown', five_hour_pct: null, weekly_pct: null }),
        ]),
      ],
      10,
    );
    expect(rows[0]?.fiveHour).toEqual({ pct: null, text: '—', tone: 'inactive' });
    expect(rows[0]?.week).toEqual({ pct: null, text: '—', tone: 'inactive' });
  });

  test('a null-source unknown pool shows dashes, not ledger', () => {
    const rows = quotaRows(
      [
        accountWith([
          poolWith({ status: 'unknown', five_hour_pct: null, weekly_pct: null, source: null }),
        ]),
      ],
      10,
    );
    expect(rows[0]?.kind).toBe('pool');
    expect(rows[0]?.fiveHour).toEqual({ pct: null, text: '—', tone: 'inactive' });
    expect(rows[0]?.week).toEqual({ pct: null, text: '—', tone: 'inactive' });
  });
});

describe('barCells', () => {
  test('fills in proportion', () => {
    expect(barCells(64, 10)).toEqual({ filled: 6, empty: 4 });
    expect(barCells(0, 10)).toEqual({ filled: 0, empty: 10 });
    expect(barCells(100, 10)).toEqual({ filled: 10, empty: 0 });
    expect(barCells(50, 9)).toEqual({ filled: 5, empty: 4 });
  });

  test('clamps and handles a narrow bar', () => {
    expect(barCells(150, 8)).toEqual({ filled: 8, empty: 0 });
    expect(barCells(-10, 8)).toEqual({ filled: 0, empty: 8 });
    expect(barCells(12, 8)).toEqual({ filled: 1, empty: 7 });
    expect(barCells(12, 0)).toEqual({ filled: 0, empty: 0 });
  });
});

describe('laneRuns', () => {
  const window = { from: 0, to: 1000 };

  test('draws each segment with its tone and merges cells', () => {
    const runs = laneRuns(
      [
        lane('queued', 0, 200),
        lane('running', 200, 800),
        lane('waiting', 800, 1000),
      ],
      window,
      10,
      1000,
    );
    expect(runs).toEqual([
      { text: '··', tone: 'queued' },
      { text: '██████', tone: 'running' },
      { text: '░░', tone: 'waiting' },
    ]);
  });

  test('fills the time before the first segment with queued dots', () => {
    const runs = laneRuns([lane('running', 300, 1000)], window, 10, 1000);
    expect(runs[0]).toEqual({ text: '···', tone: 'queued' });
    expect(runs[1]).toEqual({ text: '███████', tone: 'running' });
  });

  test('resolves an open segment at now and trims the gap after it', () => {
    const runs = laneRuns([lane('running', 0, null)], window, 10, 500);
    expect(runs).toEqual([{ text: '█████', tone: 'running' }]);
  });

  test('trims trailing blanks so the end mark follows the bar', () => {
    const runs = laneRuns([lane('running', 0, 600)], window, 10, 1000);
    expect(runs).toEqual([{ text: '██████', tone: 'running' }]);
  });

  test('a final segment shorter than a cell still shows once', () => {
    const runs = laneRuns(
      [lane('running', 0, 950), lane('waiting', 950, 1000)],
      window,
      10,
      1000,
    );
    expect(runs[runs.length - 1]).toEqual({ text: '░', tone: 'waiting' });
  });

  test('ignores zero-length segments', () => {
    const runs = laneRuns(
      [lane('running', 0, 500), lane('stuck', 500, 500), lane('running', 500, 1000)],
      window,
      10,
      1000,
    );
    expect(runs).toEqual([{ text: '██████████', tone: 'running' }]);
  });

  test('draws a review segment in the review tone', () => {
    const runs = laneRuns([lane('review', 0, 1000)], window, 10, 1000);
    expect(runs).toEqual([{ text: '▓▓▓▓▓▓▓▓▓▓', tone: 'review' }]);
  });

  test('narrow widths still yield one run', () => {
    const runs = laneRuns([lane('running', 0, 1000)], { from: 0, to: 1000 }, 20, 1000);
    expect(runs).toEqual([{ text: '████████████████████', tone: 'running' }]);
  });

  test('nothing to draw is an empty list', () => {
    expect(laneRuns([], window, 10, 1000)).toEqual([]);
    expect(laneRuns([lane('running', 0, 1000)], window, 0, 1000)).toEqual([]);
    expect(laneRuns([lane('running', 0, 1000)], { from: 10, to: 10 }, 10, 1000)).toEqual([]);
    expect(laneRuns([lane('running', -100, 0)], window, 10, 1000)).toEqual([]);
  });
});

describe('laneSpans', () => {
  const window = { from: 0, to: 1000 };

  test('turns segments and gaps into percentages summing to 100', () => {
    const spans = laneSpans(
      [lane('running', 200, 500), lane('review', 500, 700)],
      window,
      1000,
    );
    expect(spans.map((span) => span.tone)).toEqual([null, 'running', 'review', null]);
    expect(spans.map((span) => Math.round(span.pct))).toEqual([20, 30, 20, 30]);
    const total = spans.reduce((sum, span) => sum + span.pct, 0);
    expect(Math.abs(total - 100) < 1e-9).toBe(true);
  });

  test('merges adjacent equal tones', () => {
    const spans = laneSpans(
      [lane('running', 0, 500), lane('running', 500, 1000)],
      window,
      1000,
    );
    expect(spans).toEqual([{ tone: 'running', pct: 100 }]);
  });

  test('resolves an open segment at now and leaves the rest a gap', () => {
    const spans = laneSpans([lane('running', 0, null)], window, 500);
    expect(spans).toEqual([
      { tone: 'running', pct: 50 },
      { tone: null, pct: 50 },
    ]);
  });

  test('no segments is one null span', () => {
    expect(laneSpans([], window, 1000)).toEqual([{ tone: null, pct: 100 }]);
  });

  test('an invalid window has no spans', () => {
    expect(laneSpans([lane('running', 0, 1000)], { from: 5, to: 5 }, 1000)).toEqual([]);
  });
});

describe('runRows', () => {
  test('draws parents before children with tree prefixes', () => {
    const rows = runRows(busySnapshot().runs);
    expect(rows.map((row) => row.run.id)).toEqual([
      'e3-core',
      'e4-sales',
      'fix-ui',
      'e4-rpt',
      'e4-e2e',
    ]);
    expect(rows.map((row) => row.prefix)).toEqual([
      '├ ',
      '├ ',
      '│ └ ',
      '├ ',
      '└ ',
    ]);
    expect(rows.map((row) => row.depth)).toEqual([1, 1, 2, 1, 1]);
  });

  test('treats a missing parent as a root', () => {
    const rows = runRows([runWith({ id: 'a', parent_run: 'gone' })]);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.prefix).toBe('└ ');
  });

  test('labels an unnamed run with the first 8 characters of its id', () => {
    const rows = runRows([runWith({ id: 'abcdefghijk', name: null })]);
    expect(rows[0]?.label).toBe('abcdefgh');
    expect(runLabel(runWith({ id: 'abcdefghijk', name: null }))).toBe('abcdefgh');
  });

  test('visits a cycle once', () => {
    const rows = runRows([
      runWith({ id: 'a', parent_run: 'b' }),
      runWith({ id: 'b', parent_run: 'a' }),
    ]);
    expect(rows.map((row) => row.run.id).sort()).toEqual(['a', 'b']);
  });

  test('no runs is no rows', () => {
    expect(runRows([])).toEqual([]);
  });
});

describe('formatDuration', () => {
  test('formats seconds, minutes and hours', () => {
    expect(formatDuration(0)).toBe('0s');
    expect(formatDuration(1500)).toBe('1s');
    expect(formatDuration(45000)).toBe('45s');
    expect(formatDuration(59999)).toBe('59s');
    expect(formatDuration(60000)).toBe('1m');
    expect(formatDuration(240000)).toBe('4m');
    expect(formatDuration(3599000)).toBe('59m');
    expect(formatDuration(3600000)).toBe('1h00m');
    expect(formatDuration(3960000)).toBe('1h06m');
  });

  test('guards bad input', () => {
    expect(formatDuration(-1)).toBe('0s');
    expect(formatDuration(Number.NaN)).toBe('0s');
    expect(formatDuration(Number.POSITIVE_INFINITY)).toBe('0s');
  });
});

describe('labelWidth', () => {
  test('clamps 30 % of the columns to 8..12', () => {
    expect(labelWidth(0)).toBe(8);
    expect(labelWidth(20)).toBe(8);
    expect(labelWidth(30)).toBe(9);
    expect(labelWidth(39)).toBe(11);
    expect(labelWidth(40)).toBe(12);
    expect(labelWidth(100)).toBe(12);
  });
});

describe('axisLabels', () => {
  test('places the left, middle and now labels', () => {
    const line = axisLabels(60, 20);
    expect(line).toHaveLength(20);
    expect(line.startsWith('-60m')).toBe(true);
    expect(line.endsWith('now')).toBe(true);
    expect(line).toContain('-30m');
  });

  test('uses half the window for the middle label', () => {
    expect(axisLabels(30, 20)).toContain('-15m');
  });

  test('drops a label that does not fit', () => {
    const line = axisLabels(60, 8);
    expect(line).toHaveLength(8);
    expect(line.startsWith('-60m')).toBe(true);
    expect(line.endsWith('now')).toBe(true);
    expect(line.includes('-30m')).toBe(false);
    expect(axisLabels(60, 0)).toBe('');
  });
});

describe('bandText', () => {
  test('lists only what needs attention', () => {
    const text = bandText(
      { running: 2, claude: 1, user: 1, stuck: 1, failed: 1 },
      [emptyRow('codex'), poolRow('agy-acc1', 12, 80)],
    );
    expect(text).toBe(
      'SO  ◆ 1 review  ● 1 you  ⚠ 1 stuck  ✗ 1 failed  · low: agy-acc1 12%',
    );
  });

  test('is null when nothing needs attention', () => {
    expect(bandText(zeroCounts(), [poolRow('acc', 80, 70)])).toBeNull();
    expect(bandText(zeroCounts(), [])).toBeNull();
  });

  test('reports the single lowest pool under 15', () => {
    const text = bandText(
      { ...zeroCounts(), failed: 2 },
      [
        poolRow('a', 14, 80),
        poolRow('b', 3, 90),
        poolRow('c', 80, 80),
        emptyRow('d'),
      ],
    );
    expect(text).toBe('SO  ✗ 2 failed  · low: b 3%');
  });

  test('a low focus alone still carries the prefix', () => {
    expect(bandText(zeroCounts(), [poolRow('a', 5, 5)])).toBe('SO  · low: a 5%');
  });

  test('rounds a low pool percent', () => {
    expect(bandText(zeroCounts(), [poolRow('a', 14.5, 80)])).toBe('SO  · low: a 15%');
  });
});

describe('statusText', () => {
  test('shows only non-zero counts', () => {
    expect(
      statusText({ running: 2, claude: 1, user: 0, stuck: 1, failed: 0 }, true, true, true),
    ).toBe('SO ▶2 ◆1 ⚠1');
    expect(
      statusText({ running: 0, claude: 0, user: 1, stuck: 0, failed: 2 }, true, true, true),
    ).toBe('SO ●1 ✗2');
  });

  test('says down only with active runs', () => {
    expect(statusText(zeroCounts(), false, true, true)).toBe('SO down');
    expect(statusText(zeroCounts(), false, false, true)).toBe('SO');
  });

  test('is null without runs in the window', () => {
    expect(statusText(zeroCounts(), true, false, false)).toBeNull();
    expect(statusText(zeroCounts(), false, true, false)).toBeNull();
  });
});

describe('newAttention', () => {
  const now = at('2026-10-25T14:32:05Z');

  test('says nothing on the first poll', () => {
    expect(newAttention(null, { snapshot: busySnapshot(), orchestrator: [] })).toEqual([]);
  });

  test('says nothing when attention did not change', () => {
    const snapshot = busySnapshot();
    expect(
      newAttention(
        { snapshot, orchestrator: [lane('idle', now - 1000, null)] },
        { snapshot: busySnapshot(), orchestrator: [lane('idle', now - 1000, null)] },
      ),
    ).toEqual([]);
  });

  test('announces each newly attentive run once', () => {
    const before = emptySnapshot();
    before.runs = [
      runWith({ id: 'a', name: 'e3-core', attention: null }),
      runWith({ id: 'b', name: 'e4-sales', attention: 'claude' }),
      runWith({ id: 'c', name: 'gone', attention: 'stuck' }),
    ];
    const after = busySnapshot();
    after.runs = [
      runWith({ id: 'a', name: 'e3-core', attention: 'claude' }),
      runWith({ id: 'b', name: 'e4-sales', attention: 'stuck' }),
      runWith({ id: 'd', name: 'e4-e2e', attention: 'failed' }),
    ];
    expect(
      newAttention(
        { snapshot: before, orchestrator: [] },
        { snapshot: after, orchestrator: [] },
      ),
    ).toEqual([
      'SO: e3-core needs review',
      'SO: e4-sales is stuck',
      'SO: e4-e2e failed',
    ]);
  });

  test('labels an unnamed run in its toast with the first 8 characters of its id', () => {
    const before = emptySnapshot();
    before.runs = [runWith({ id: 'abcdefghijk', name: null, attention: null })];
    const after = emptySnapshot();
    after.runs = [runWith({ id: 'abcdefghijk', name: null, attention: 'failed' })];
    expect(
      newAttention(
        { snapshot: before, orchestrator: [] },
        { snapshot: after, orchestrator: [] },
      ),
    ).toEqual(['SO: abcdefgh failed']);
  });

  test('announces Claude needing the person when the lane turns user', () => {
    const snapshot = busySnapshot();
    expect(
      newAttention(
        { snapshot, orchestrator: [lane('claude', now - 1000, now - 500), lane('idle', now - 500, null)] },
        { snapshot: busySnapshot(), orchestrator: [lane('claude', now - 1000, now - 500), lane('user', now - 500, null)] },
      ),
    ).toEqual(['SO: Claude needs you']);

    expect(
      newAttention(
        { snapshot, orchestrator: [lane('user', now - 1000, null)] },
        { snapshot: busySnapshot(), orchestrator: [lane('user', now - 1000, null)] },
      ),
    ).toEqual([]);
  });
});
