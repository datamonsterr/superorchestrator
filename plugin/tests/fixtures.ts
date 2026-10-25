// Snapshot fixtures for the model tests. Each factory returns a fresh object,
// so a test may change what it needs without touching another test.

import type { Account, Pool, Run, RunSegment, Snapshot } from '../types';

const NOW = '2026-10-25T14:32:05Z';
const FROM = '2026-10-25T13:32:05Z';

function pool(over: Partial<Pool>): Pool {
  return {
    pool: 'default',
    status: 'ok',
    five_hour_pct: 64,
    five_hour_resets_at: '2026-10-25T16:00:00Z',
    weekly_pct: 51,
    weekly_resets_at: '2026-10-29T00:00:00Z',
    cooldown_until: null,
    source: 'probe',
    observed_at: '2026-10-25T14:30:00Z',
    ...over,
  };
}

function account(over: Partial<Account>): Account {
  return {
    name: 'codex-acc1',
    harness: 'codex',
    enabled: true,
    signed_in: 'yes',
    active_runs: 0,
    cap: 2,
    pools: [pool({})],
    ...over,
  };
}

function segment(state: RunSegment['state'], from: string, to: string | null): RunSegment {
  return { state, from, to };
}

function run(over: Partial<Run>): Run {
  return {
    id: 'run',
    name: 'run',
    parent_run: null,
    depth: 1,
    harness: 'codex',
    account: 'codex-acc1',
    model: 'codex/gpt-6.1-sol/high',
    state: 'running',
    attention: null,
    question: null,
    created_at: '2026-10-25T14:27:30Z',
    started_at: '2026-10-25T14:27:30Z',
    finished_at: null,
    segments: [],
    marks: [],
    ...over,
  };
}

function okAccounts(): Account[] {
  return [
    account({ name: 'ok1', pools: [pool({ five_hour_pct: 88, weekly_pct: 77 })] }),
    account({ name: 'ok2', pools: [pool({ five_hour_pct: 85, weekly_pct: 74 })] }),
    account({ name: 'ok3', pools: [pool({ five_hour_pct: 82, weekly_pct: 71 })] }),
    account({ name: 'ok4', pools: [pool({ five_hour_pct: 79, weekly_pct: 68 })] }),
    account({ name: 'ok5', pools: [pool({ five_hour_pct: 76, weekly_pct: 65 })] }),
  ];
}

/** The layout the monitor draws: one busy session, mixed quota, five runs. */
export function busySnapshot(): Snapshot {
  return {
    now: NOW,
    window: { from: FROM, to: NOW },
    supervisor: {
      running: true,
      pid: 1644957,
      uptime_s: 3965,
      stale: false,
      warning: null,
      active_runs: 10,
      sessions: 34,
    },
    accounts: [
      account({ name: 'codex-acc1', pools: [pool({})] }),
      account({
        name: 'codex-acc2',
        harness: 'codex',
        pools: [
          pool({
            status: 'exhausted',
            five_hour_pct: 0,
            weekly_pct: 0,
            cooldown_until: '2026-10-25T13:52:00Z',
            source: 'event',
          }),
        ],
      }),
      account({
        name: 'agy-acc1',
        harness: 'agy',
        pools: [
          pool({ pool: 'gemini', status: 'low', five_hour_pct: 12, weekly_pct: 80 }),
          pool({ pool: 'default', five_hour_pct: 90, weekly_pct: 88 }),
        ],
      }),
      ...okAccounts(),
    ],
    session: {
      id: '01a17271-f94f-7677-9d0d-75b33c4b82f8',
      cwd: '/home/dat/dev/app',
      started_at: '2026-10-25T13:00:00Z',
      matched_by: 'pid',
    },
    runs: [
      run({
        id: 'e3-core',
        name: 'e3-core',
        state: 'pending_review',
        attention: 'claude',
        question: 'Ship the parser or wait?',
        created_at: '2026-10-25T14:27:30Z',
        started_at: '2026-10-25T14:28:00Z',
        segments: [
          segment('queued', '2026-10-25T14:27:30Z', '2026-10-25T14:28:00Z'),
          segment('running', '2026-10-25T14:28:00Z', '2026-10-25T14:30:05Z'),
          segment('pending_review', '2026-10-25T14:30:05Z', null),
        ],
        marks: [
          { ts: '2026-10-25T14:30:00Z', kind: 'check_failed' },
          { ts: '2026-10-25T14:31:00Z', kind: 'question' },
        ],
      }),
      run({
        id: 'e4-sales',
        name: 'e4-sales',
        state: 'escalated',
        attention: 'stuck',
        account: 'codex-acc3',
        created_at: '2026-10-25T14:26:05Z',
        started_at: '2026-10-25T14:26:35Z',
        segments: [
          segment('queued', '2026-10-25T14:26:05Z', '2026-10-25T14:26:35Z'),
          segment('running', '2026-10-25T14:26:35Z', '2026-10-25T14:30:05Z'),
          segment('escalated', '2026-10-25T14:30:05Z', null),
        ],
        marks: [{ ts: '2026-10-25T14:30:05Z', kind: 'escalated' }],
      }),
      run({
        id: 'fix-ui',
        name: 'fix-ui',
        parent_run: 'e4-sales',
        depth: 2,
        state: 'running',
        account: null,
        model: 'codex/gpt-6.1-sol/high',
        created_at: '2026-10-25T14:29:05Z',
        started_at: '2026-10-25T14:29:35Z',
        segments: [
          segment('queued', '2026-10-25T14:29:05Z', '2026-10-25T14:29:35Z'),
          segment('running', '2026-10-25T14:29:35Z', null),
        ],
      }),
      run({
        id: 'e4-rpt',
        name: 'e4-rpt',
        state: 'succeeded',
        finished_at: '2026-10-25T14:29:05Z',
        created_at: '2026-10-25T14:20:05Z',
        started_at: '2026-10-25T14:20:35Z',
        segments: [
          segment('queued', '2026-10-25T14:20:05Z', '2026-10-25T14:20:35Z'),
          segment('running', '2026-10-25T14:20:35Z', '2026-10-25T14:29:05Z'),
        ],
        marks: [{ ts: '2026-10-25T14:29:05Z', kind: 'review_accepted' }],
      }),
      run({
        id: 'e4-e2e',
        name: 'e4-e2e',
        state: 'failed',
        attention: 'failed',
        finished_at: '2026-10-25T14:31:35Z',
        created_at: '2026-10-25T14:30:35Z',
        started_at: '2026-10-25T14:31:05Z',
        segments: [
          segment('queued', '2026-10-25T14:30:35Z', '2026-10-25T14:31:05Z'),
          segment('running', '2026-10-25T14:31:05Z', '2026-10-25T14:31:35Z'),
        ],
        marks: [{ ts: '2026-10-25T14:31:35Z', kind: 'check_failed' }],
      }),
    ],
  };
}

/** No session, no runs, no accounts: the mod has nothing to draw. */
export function emptySnapshot(): Snapshot {
  return {
    now: NOW,
    window: { from: FROM, to: NOW },
    supervisor: {
      running: true,
      pid: 1644957,
      uptime_s: 3965,
      stale: false,
      warning: null,
      active_runs: 0,
      sessions: 0,
    },
    accounts: [],
    session: null,
    runs: [],
  };
}

/** The supervisor is down, but this session still waits on a running run. */
export function downSnapshot(): Snapshot {
  return {
    now: NOW,
    window: { from: FROM, to: NOW },
    supervisor: {
      running: false,
      pid: null,
      uptime_s: null,
      stale: null,
      warning: null,
      active_runs: null,
      sessions: null,
    },
    accounts: [],
    session: {
      id: '01a17271-f94f-7677-9d0d-75b33c4b82f8',
      cwd: '/home/dat/dev/app',
      started_at: '2026-10-25T13:00:00Z',
      matched_by: 'pid',
    },
    runs: [
      run({
        id: 'still-running',
        name: 'still-running',
        state: 'running',
        started_at: '2026-10-25T14:30:00Z',
        segments: [segment('running', '2026-10-25T14:30:00Z', null)],
      }),
    ],
  };
}
