// The superorchestrator monitor mod's contract.
//
// The snapshot types mirror, field for field, the JSON `superorchestrator
// monitor --json` prints (snake_case, missing values null). The drawing types
// are what the mod keeps in `$.state` and what the pure model hands to the
// renderer.

/** The supervisor is up, or the command could not reach it. */
export type Supervisor = {
  running: boolean;
  pid: number | null;
  uptime_s: number | null;
  stale: boolean | null;
  warning: string | null;
  active_runs: number | null;
  sessions: number | null;
};

export type SnapshotWindow = {
  from: string;
  to: string;
};

export type PoolStatus = 'ok' | 'low' | 'exhausted' | 'unknown';
export type PoolSource = 'probe' | 'event' | 'ledger';
export type SignedInState = 'yes' | 'no' | 'unknown';

/** One quota window of one account's pool. */
export type Pool = {
  pool: string;
  status: PoolStatus;
  five_hour_pct: number | null;
  five_hour_resets_at: string | null;
  weekly_pct: number | null;
  weekly_resets_at: string | null;
  cooldown_until: string | null;
  source: PoolSource;
  observed_at: string | null;
};

export type Account = {
  name: string;
  harness: string;
  enabled: boolean;
  signed_in: SignedInState;
  active_runs: number;
  cap: number;
  pools: Pool[];
};

/** Where this Claude Code session stands, or null when none matched. */
export type Session = {
  id: string;
  cwd: string;
  started_at: string;
  matched_by: string;
} | null;

export type RunState =
  | 'queued'
  | 'starting'
  | 'running'
  | 'waiting'
  | 'stalled'
  | 'escalated'
  | 'pending_review'
  | 'succeeded'
  | 'failed'
  | 'cancelled';

/** What the command says needs a person or Claude; `user` is the mod's own. */
export type Attention = 'claude' | 'stuck' | 'failed' | null;

export type MarkKind =
  | 'question'
  | 'nudge'
  | 'escalated'
  | 'failover'
  | 'check_failed'
  | 'review_rejected'
  | 'review_accepted';

export type RunSegment = {
  state: RunState;
  from: string;
  to: string | null;
};

export type RunMark = {
  ts: string;
  kind: MarkKind;
};

export type Run = {
  id: string;
  name: string;
  parent_run: string | null;
  depth: number;
  harness: string;
  account: string | null;
  model: string | null;
  state: RunState;
  attention: Attention;
  question: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  segments: RunSegment[];
  marks: RunMark[];
};

/** One poll of `superorchestrator monitor --json`. */
export type Snapshot = {
  now: string;
  window: SnapshotWindow;
  supervisor: Supervisor;
  accounts: Account[];
  session: Session;
  runs: Run[];
};

/**
 * What a lane segment shows.
 *
 * `review` is a run waiting for Claude (pending review, or waiting with a
 * question): it shares the `claude` color and end mark but draws `▓` rather
 * than the orchestrator's own `█`. Keeping it a tone of its own lets a lane
 * merge cells and lets the renderer pick the glyph without a per-segment
 * override.
 */
export type Tone =
  | 'running'
  | 'claude'
  | 'review'
  | 'user'
  | 'stuck'
  | 'failed'
  | 'done'
  | 'waiting'
  | 'queued'
  | 'idle'
  | 'cancelled';

/** One stretch of a lane, epoch milliseconds; `to` null means still open. */
export type LaneSegment = {
  tone: Tone;
  from: number;
  to: number | null;
};

/** The part of the timeline a lane is drawn over, epoch milliseconds. */
export type LaneWindow = {
  from: number;
  to: number;
};

export type ThemeInfo = {
  mode: 'dark' | 'light';
  ansi: boolean;
  daltonized: boolean;
};

export type FetchState = {
  status: 'idle' | 'ok' | 'down' | 'missing' | 'error';
  at: number;
  error: string | null;
};

/** The colors a quota cell uses; all are Claude Code theme keys. */
export type QuotaTone = 'success' | 'warning' | 'error' | 'inactive';

export type QuotaCell = {
  /** Headroom percent, 0-100, or null when unknown. */
  pct: number | null;
  /** What the cell shows: `64%`, `—`, ... */
  text: string;
  tone: QuotaTone;
};

export type QuotaRowKind = 'pool' | 'cooldown' | 'exhausted' | 'ledger' | 'folded';

/**
 * One line of the quota block.
 *
 * A `pool` row draws the two cells and, when `note` is set, its trailing flag
 * (`low`). A `cooldown`, `exhausted`, `ledger` or `folded` row draws `note`
 * (or, for `folded`, its label) as the whole row instead of the cells.
 */
export type QuotaRow = {
  label: string;
  kind: QuotaRowKind;
  fiveHour: QuotaCell;
  week: QuotaCell;
  note: string | null;
  noteTone: QuotaTone | null;
};

/** The counts the counts line, band and status line share. */
export type AttentionCounts = {
  running: number;
  claude: number;
  user: number;
  stuck: number;
  failed: number;
};

/** One rendered piece of a terminal lane: its cells and their tone. */
export type LaneRun = {
  text: string;
  tone: Tone;
};

/** One proportional piece of a desktop lane; a gap is a null tone. */
export type LaneSpan = {
  tone: Tone | null;
  /** Percent of the window, 0-100. */
  pct: number;
};

/** One run's place in the tree the timeline draws. */
export type RunRow = {
  run: Run;
  /** The tree prefix, connector included, ending in a space (`│ └ `). */
  prefix: string;
  /** Tree depth, roots at 1. */
  depth: number;
};

/** What `newAttention` compares: the poll and the orchestrator lane beside it. */
export type AttentionView = {
  snapshot: Snapshot;
  orchestrator: LaneSegment[];
};

declare module 'claude-code' {
  interface PluginState {
    superorchestrator: {
      snapshot: Snapshot | null;
      fetch: FetchState;
      orchestrator: LaneSegment[];
      supervisor: LaneSegment[];
      theme: ThemeInfo;
      windowMin: number;
      expanded: string[];
      clientPid: number | null;
    };
  }
}
