// The monitor's pure model: parsing, tones, counts, quota rows and the shapes
// the two surfaces draw. No hooks, no `$`, no clock: every time is passed in.

import type { ThemeKey } from 'claude-code';
import type {
  Account,
  Attention,
  AttentionCounts,
  AttentionView,
  LaneRun,
  LaneSegment,
  LaneSpan,
  LaneWindow,
  Pool,
  QuotaCell,
  QuotaRow,
  QuotaTone,
  Run,
  RunRow,
  RunState,
  Snapshot,
  Supervisor,
  SupervisorTone,
  ThemeInfo,
  Tone,
} from '../types';

// ---------------------------------------------------------------------------
// Snapshot parsing
// ---------------------------------------------------------------------------

const RUN_STATES: readonly RunState[] = [
  'queued',
  'starting',
  'running',
  'waiting',
  'stalled',
  'escalated',
  'pending_review',
  'succeeded',
  'failed',
  'cancelled',
];

const ATTENTIONS = ['claude', 'stuck', 'failed'] as const;

const MARK_KINDS = [
  'question',
  'nudge',
  'escalated',
  'failover',
  'check_failed',
  'review_rejected',
  'review_accepted',
] as const;

const POOL_STATUSES = ['ok', 'low', 'exhausted', 'unknown'] as const;
const POOL_SOURCES = ['probe', 'event', 'ledger'] as const;
const SIGNED_IN = ['yes', 'no', 'unknown'] as const;

type Fields = Record<string, unknown>;

function isRecord(value: unknown): value is Fields {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function has(object: Fields, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(object, key);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function isInteger(value: unknown, min = 0): value is number {
  return isNumber(value) && Number.isInteger(value) && value >= min;
}

/** A timestamp the drawing can turn into time; the contract says RFC 3339 UTC. */
function isTime(value: unknown): value is string {
  return isString(value) && !Number.isNaN(Date.parse(value));
}

function isNullable(value: unknown, test: (candidate: unknown) => boolean): boolean {
  return value === null || test(value);
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return isString(value) && (allowed as readonly string[]).includes(value);
}

function isPercent(value: unknown): value is number {
  return isInteger(value) && value <= 100;
}

function isWindow(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'from') &&
    has(value, 'to') &&
    isTime(value['from']) &&
    isTime(value['to'])
  );
}

function isSupervisor(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'running') &&
    has(value, 'pid') &&
    has(value, 'uptime_s') &&
    has(value, 'stale') &&
    has(value, 'warning') &&
    has(value, 'active_runs') &&
    has(value, 'sessions') &&
    typeof value['running'] === 'boolean' &&
    isNullable(value['pid'], (v) => isInteger(v, 1)) &&
    isNullable(value['uptime_s'], (v) => isInteger(v)) &&
    isNullable(value['stale'], (v) => typeof v === 'boolean') &&
    isNullable(value['warning'], isString) &&
    isNullable(value['active_runs'], (v) => isInteger(v)) &&
    isNullable(value['sessions'], (v) => isInteger(v))
  );
}

function isPool(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'pool') &&
    has(value, 'status') &&
    has(value, 'five_hour_pct') &&
    has(value, 'five_hour_resets_at') &&
    has(value, 'weekly_pct') &&
    has(value, 'weekly_resets_at') &&
    has(value, 'cooldown_until') &&
    has(value, 'source') &&
    has(value, 'observed_at') &&
    isString(value['pool']) &&
    oneOf(value['status'], POOL_STATUSES) &&
    isNullable(value['five_hour_pct'], isPercent) &&
    isNullable(value['five_hour_resets_at'], isTime) &&
    isNullable(value['weekly_pct'], isPercent) &&
    isNullable(value['weekly_resets_at'], isTime) &&
    isNullable(value['cooldown_until'], isTime) &&
    isNullable(value['source'], (candidate) => oneOf(candidate, POOL_SOURCES)) &&
    isNullable(value['observed_at'], isTime)
  );
}

function isAccount(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'name') &&
    has(value, 'harness') &&
    has(value, 'enabled') &&
    has(value, 'signed_in') &&
    has(value, 'active_runs') &&
    has(value, 'cap') &&
    has(value, 'pools') &&
    isString(value['name']) &&
    isString(value['harness']) &&
    typeof value['enabled'] === 'boolean' &&
    oneOf(value['signed_in'], SIGNED_IN) &&
    isInteger(value['active_runs']) &&
    isInteger(value['cap']) &&
    Array.isArray(value['pools']) &&
    value['pools'].every(isPool)
  );
}

function isSession(value: unknown): boolean {
  return (
    value === null ||
    (isRecord(value) &&
      has(value, 'id') &&
      has(value, 'cwd') &&
      has(value, 'started_at') &&
      has(value, 'matched_by') &&
      isString(value['id']) &&
      isNullable(value['cwd'], isString) &&
      isTime(value['started_at']) &&
      isString(value['matched_by']))
  );
}

function isSegment(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'state') &&
    has(value, 'from') &&
    has(value, 'to') &&
    oneOf(value['state'], RUN_STATES) &&
    isTime(value['from']) &&
    isNullable(value['to'], isTime)
  );
}

function isMark(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'ts') &&
    has(value, 'kind') &&
    isTime(value['ts']) &&
    oneOf(value['kind'], MARK_KINDS)
  );
}

function isRun(value: unknown): boolean {
  return (
    isRecord(value) &&
    has(value, 'id') &&
    has(value, 'name') &&
    has(value, 'parent_run') &&
    has(value, 'depth') &&
    has(value, 'harness') &&
    has(value, 'account') &&
    has(value, 'model') &&
    has(value, 'state') &&
    has(value, 'attention') &&
    has(value, 'question') &&
    has(value, 'created_at') &&
    has(value, 'started_at') &&
    has(value, 'finished_at') &&
    has(value, 'segments') &&
    has(value, 'marks') &&
    isString(value['id']) &&
    isNullable(value['name'], isString) &&
    isNullable(value['parent_run'], isString) &&
    isInteger(value['depth']) &&
    isString(value['harness']) &&
    isNullable(value['account'], isString) &&
    isNullable(value['model'], isString) &&
    oneOf(value['state'], RUN_STATES) &&
    isNullable(value['attention'], (candidate) => oneOf(candidate, ATTENTIONS)) &&
    isNullable(value['question'], isString) &&
    isTime(value['created_at']) &&
    isNullable(value['started_at'], isTime) &&
    isNullable(value['finished_at'], isTime) &&
    Array.isArray(value['segments']) &&
    value['segments'].every(isSegment) &&
    Array.isArray(value['marks']) &&
    value['marks'].every(isMark)
  );
}

function isSnapshot(value: unknown): value is Snapshot {
  return (
    isRecord(value) &&
    has(value, 'now') &&
    has(value, 'window') &&
    has(value, 'supervisor') &&
    has(value, 'accounts') &&
    has(value, 'session') &&
    has(value, 'runs') &&
    isTime(value['now']) &&
    isWindow(value['window']) &&
    isSupervisor(value['supervisor']) &&
    Array.isArray(value['accounts']) &&
    value['accounts'].every(isAccount) &&
    isSession(value['session']) &&
    Array.isArray(value['runs']) &&
    value['runs'].every(isRun)
  );
}

/**
 * Reads one poll's JSON.
 *
 * Defensive by design: bad JSON, a missing field, or a value of the wrong
 * type (including a timestamp that does not parse) is null, because the mod
 * draws the last good snapshot instead of failing a hook.
 */
export function parseSnapshot(text: string): Snapshot | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  return isSnapshot(value) ? value : null;
}

// ---------------------------------------------------------------------------
// Tones
// ---------------------------------------------------------------------------

/**
 * How one tone draws.
 *
 * `review` is the run-side half of the `claude` tone: a run pending review or
 * waiting with a question for Claude uses it, so lanes can merge cells and the
 * renderer picks `▓` and `◆` without a per-segment override. The orchestrator's
 * own working segments use `claude` with `█`.
 */
export type ToneStyle = {
  theme: ThemeKey;
  bar: string | null;
  mark: string | null;
  dim: boolean;
};

export const TONES: Record<Tone, ToneStyle> = {
  running: { theme: 'ide', bar: '█', mark: '▶', dim: false },
  claude: { theme: 'claude', bar: '█', mark: '◆', dim: false },
  review: { theme: 'claude', bar: '▓', mark: '◆', dim: false },
  user: { theme: 'bashBorder', bar: '█', mark: '●', dim: false },
  stuck: { theme: 'warning', bar: '▒', mark: '⚠', dim: false },
  failed: { theme: 'error', bar: null, mark: '✗', dim: false },
  done: { theme: 'success', bar: null, mark: '✓', dim: false },
  waiting: { theme: 'inactive', bar: '░', mark: null, dim: false },
  queued: { theme: 'inactive', bar: '·', mark: '○', dim: true },
  idle: { theme: 'subtle', bar: '░', mark: null, dim: false },
  cancelled: { theme: 'inactive', bar: null, mark: '■', dim: false },
  // supervisor tones: the supervisor lane's own three styles
  up: { theme: 'success', bar: '█', mark: null, dim: false },
  stale: { theme: 'warning', bar: '▒', mark: null, dim: false },
  down: { theme: 'error', bar: '·', mark: null, dim: false },
};

const STATE_TONES: Record<RunState, Tone> = {
  queued: 'queued',
  starting: 'running',
  running: 'running',
  waiting: 'waiting',
  stalled: 'stuck',
  escalated: 'stuck',
  pending_review: 'review',
  succeeded: 'done',
  failed: 'failed',
  cancelled: 'cancelled',
};

/**
 * The tone a run's state draws.
 *
 * A question for the orchestrator (`hasQuestionForOrchestrator`) turns any
 * live state into `review`; a terminal or stuck run keeps its own tone because
 * finishing or escalating is the more specific fact.
 */
export function toneOfRunState(state: RunState, hasQuestionForOrchestrator: boolean): Tone {
  if (
    hasQuestionForOrchestrator &&
    (state === 'queued' ||
      state === 'starting' ||
      state === 'running' ||
      state === 'waiting' ||
      state === 'pending_review')
  ) {
    return 'review';
  }
  return STATE_TONES[state];
}

/**
 * The theme a `theme` setting names. Anything the set does not list is dark.
 */
export function themeInfo(value: string | null | undefined): ThemeInfo {
  switch (value) {
    case 'light':
      return { mode: 'light', ansi: false, daltonized: false };
    case 'light-daltonized':
      return { mode: 'light', ansi: false, daltonized: true };
    case 'light-ansi':
      return { mode: 'light', ansi: true, daltonized: false };
    case 'dark-daltonized':
      return { mode: 'dark', ansi: false, daltonized: true };
    case 'dark-ansi':
      return { mode: 'dark', ansi: true, daltonized: false };
    default:
      return { mode: 'dark', ansi: false, daltonized: false };
  }
}

/**
 * The color of an empty track or headroom cell: `subtle` on dark themes, dim
 * `inactive` on light ones.
 */
export function trackColor(theme: ThemeInfo): { color: ThemeKey; dim: boolean } {
  return theme.mode === 'light'
    ? { color: 'inactive', dim: true }
    : { color: 'subtle', dim: false };
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

/** The supervisor lane's tone for one poll: down, stale or up. */
export function supervisorTone(supervisor: Supervisor): SupervisorTone {
  if (!supervisor.running) return 'down';
  return supervisor.stale === true ? 'stale' : 'up';
}

function openTone(lane: readonly LaneSegment[], now: number): Tone | null {
  let tone: Tone | null = null;
  for (const segment of lane) {
    if (segment.from <= now && (segment.to === null || segment.to > now)) {
      tone = segment.tone;
    }
  }
  return tone;
}

/**
 * What the counts line, the band and the status line share.
 *
 * `running`, `claude`, `stuck` and `failed` count runs by their `attention`
 * first, then by the tone they draw at `now`; `user` is 1 while the
 * orchestrator lane's open segment is `user`.
 */
export function attentionCounts(
  snapshot: Snapshot,
  orchestratorLane: readonly LaneSegment[],
  now: number,
): AttentionCounts {
  const counts: AttentionCounts = { running: 0, claude: 0, user: 0, stuck: 0, failed: 0 };
  for (const run of snapshot.runs) {
    const tone = toneOfRunState(run.state, run.question !== null);
    if (run.attention === 'claude') counts.claude += 1;
    else if (run.attention === 'stuck') counts.stuck += 1;
    else if (run.attention === 'failed') counts.failed += 1;
    else if (tone === 'review') counts.claude += 1;
    else if (tone === 'stuck') counts.stuck += 1;
    else if (tone === 'failed') counts.failed += 1;
    else if (tone === 'running') counts.running += 1;
  }
  if (openTone(orchestratorLane, now) === 'user') counts.user = 1;
  return counts;
}

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------

function quotaTone(pct: number | null): QuotaTone {
  if (pct === null) return 'inactive';
  if (pct >= 50) return 'success';
  if (pct >= 15) return 'warning';
  return 'error';
}

function percentCell(pct: number | null): QuotaCell {
  return pct === null
    ? { pct: null, text: '—', tone: 'inactive' }
    : { pct, text: `${pct}%`, tone: quotaTone(pct) };
}

function minPercent(pool: Pool): number {
  const known = [pool.five_hour_pct, pool.weekly_pct].filter((pct): pct is number => pct !== null);
  return known.length === 0 ? Number.POSITIVE_INFINITY : Math.min(...known);
}

/**
 * The account's pool name shortened to its first letters: one letter unless
 * another pool of the same account starts the same way, then up to four.
 */
function shortPoolName(name: string, account: Account): string {
  let length = 1;
  while (length < name.length && length < 4) {
    const prefix = name.slice(0, length);
    const collides = account.pools.some(
      (other) => other.pool !== name && other.pool.slice(0, length) === prefix,
    );
    if (!collides) break;
    length += 1;
  }
  return name.slice(0, length) || name;
}

type QuotaEntry = {
  pool: Pool;
  label: string;
  minimum: number;
  rank: number;
  foldable: boolean;
};

function quotaEntry(account: Account, pool: Pool): QuotaEntry {
  const minimum = minPercent(pool);
  const bothKnown =
    pool.five_hour_pct !== null && pool.weekly_pct !== null && minimum >= 50;
  const foldable =
    pool.status === 'ok' && bothKnown && pool.cooldown_until === null;
  let rank: number;
  if (pool.status === 'exhausted') rank = 0;
  else if (pool.cooldown_until !== null) rank = 1;
  else if (pool.status === 'low' || minimum < 50) rank = 2;
  else if (pool.status === 'unknown' || !Number.isFinite(minimum)) rank = 4;
  else rank = 3;
  return {
    pool,
    label:
      account.pools.length > 1
        ? `${account.name}/${shortPoolName(pool.pool, account)}`
        : account.name,
    minimum,
    rank,
    foldable,
  };
}

function clockText(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '--:--';
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

function noteRow(
  label: string,
  kind: QuotaRow['kind'],
  note: string,
  tone: QuotaTone,
  pool: Pool,
): QuotaRow {
  return {
    label,
    kind,
    fiveHour: percentCell(pool.five_hour_pct),
    week: percentCell(pool.weekly_pct),
    note,
    noteTone: tone,
  };
}

function quotaRow(entry: QuotaEntry): QuotaRow {
  const { pool, label } = entry;
  if (pool.cooldown_until !== null) {
    return noteRow(label, 'cooldown', `cooldown until ${clockText(pool.cooldown_until)}`, 'error', pool);
  }
  if (pool.status === 'exhausted') return noteRow(label, 'exhausted', 'exhausted', 'error', pool);
  if (pool.source === 'ledger' && pool.five_hour_pct === null && pool.weekly_pct === null) {
    return noteRow(label, 'ledger', 'ledger', 'inactive', pool);
  }
  return {
    label,
    kind: 'pool',
    fiveHour: percentCell(pool.five_hour_pct),
    week: percentCell(pool.weekly_pct),
    note: pool.status === 'low' ? 'low' : null,
    noteTone: pool.status === 'low' ? 'warning' : null,
  };
}

/**
 * The quota block's rows, worst first: exhausted, then cooling down, then low,
 * then the rest by the lower of the two windows.
 *
 * When the rows do not fit `maxRows`, the worst rows stay and the healthiest
 * pools (both windows at 50 % or more) fold into one `+N ok ≥50%` row. One
 * account with several pools labels each row `account/<pool's first letters>`;
 * an account with one pool uses its name alone.
 */
export function quotaRows(accounts: readonly Account[], maxRows: number): QuotaRow[] {
  if (!(maxRows > 0)) return [];
  const entries: QuotaEntry[] = [];
  for (const account of accounts) {
    for (const pool of account.pools) entries.push(quotaEntry(account, pool));
  }
  entries.sort(
    (a, b) =>
      a.rank - b.rank ||
      a.minimum - b.minimum ||
      a.label.localeCompare(b.label),
  );

  const limit = Number.isFinite(maxRows) ? Math.floor(maxRows) : entries.length;
  if (entries.length <= limit) return entries.map(quotaRow);

  const fixed = entries.filter((entry) => !entry.foldable);
  const foldable = entries.filter((entry) => entry.foldable);
  if (foldable.length === 0) return entries.slice(0, limit).map(quotaRow);

  const budget = limit - fixed.length;
  if (budget <= 0) return entries.slice(0, limit).map(quotaRow);

  const shown = budget - 1;
  const rows: QuotaRow[] = [];
  let taken = 0;
  let folded = 0;
  for (const entry of entries) {
    if (!entry.foldable) {
      rows.push(quotaRow(entry));
    } else if (taken < shown) {
      rows.push(quotaRow(entry));
      taken += 1;
    } else if (folded === 0) {
      folded = foldable.length - shown;
    }
  }
  if (folded > 0) {
    rows.push({
      label: `+${folded} ok ≥50%`,
      kind: 'folded',
      fiveHour: percentCell(null),
      week: percentCell(null),
      note: null,
      noteTone: 'success',
    });
  }
  return rows.slice(0, limit);
}

// ---------------------------------------------------------------------------
// Bars and lanes
// ---------------------------------------------------------------------------

/** How many cells of a `width`-cell bar a percent fills. */
export function barCells(pct: number, width: number): { filled: number; empty: number } {
  const cells = Math.max(0, Math.floor(width));
  if (cells === 0 || !Number.isFinite(pct)) return { filled: 0, empty: cells };
  const filled = Math.round((Math.min(100, Math.max(0, pct)) / 100) * cells);
  return { filled, empty: cells - filled };
}

type ClipSpan = {
  tone: Tone;
  from: number;
  to: number;
};

function clipSpans(
  segments: readonly LaneSegment[],
  window: LaneWindow,
  now: number,
): ClipSpan[] {
  const spans: ClipSpan[] = [];
  for (const segment of segments) {
    const to = segment.to === null ? now : segment.to;
    const from = Math.max(segment.from, window.from);
    const end = Math.min(to, window.to);
    if (end > from) spans.push({ tone: segment.tone, from, to: end });
  }
  spans.sort((a, b) => a.from - b.from || a.to - b.to);
  return spans;
}

/**
 * A terminal lane's cells, merged into runs of one tone.
 *
 * Cells are laid over the whole window. Time before the first segment is `·`
 * in the queued tone; gaps between segments and after the last bar are blank.
 * Trailing blank cells are trimmed, so the end mark can follow the last bar.
 * A last segment shorter than one cell still gets the cell it starts in.
 */
export function laneRuns(
  segments: readonly LaneSegment[],
  window: LaneWindow,
  width: number,
  now: number,
): LaneRun[] {
  const cells = Math.max(0, Math.floor(width));
  const duration = window.to - window.from;
  if (cells === 0 || !(duration > 0)) return [];

  const spans = clipSpans(segments, window, now);
  const first = spans[0];
  if (first === undefined) return [];

  const cellMs = duration / cells;
  const tones: Tone[] = [];
  const chars: string[] = [];
  for (let index = 0; index < cells; index += 1) {
    const start = window.from + index * cellMs;
    const end = start + cellMs;
    let bestTone: Tone | null = null;
    let bestOverlap = 0;
    for (const span of spans) {
      const overlap = Math.min(end, span.to) - Math.max(start, span.from);
      if (overlap > bestOverlap) {
        bestOverlap = overlap;
        bestTone = span.tone;
      }
    }
    if (bestTone !== null && bestOverlap * 2 > cellMs) {
      tones.push(bestTone);
      chars.push(TONES[bestTone].bar ?? '');
    } else if (end <= first.from) {
      tones.push('queued');
      chars.push('·');
    } else {
      tones.push('idle');
      chars.push(' ');
    }
  }

  // A final segment shorter than the cell it lands in still shows once.
  let lastBar: ClipSpan | undefined;
  for (const span of spans) {
    if (TONES[span.tone].bar !== null) lastBar = span;
  }
  if (lastBar !== undefined && lastBar.to - lastBar.from < cellMs) {
    const index = Math.min(
      cells - 1,
      Math.max(0, Math.floor((lastBar.from - window.from) / cellMs)),
    );
    tones[index] = lastBar.tone;
    chars[index] = TONES[lastBar.tone].bar ?? '';
  }

  let last = cells - 1;
  while (last >= 0 && (chars[last] === ' ' || chars[last] === '')) last -= 1;
  if (last < 0) return [];

  const runs: LaneRun[] = [];
  for (let index = 0; index <= last; index += 1) {
    const tone = tones[index];
    const text = chars[index];
    if (tone === undefined || text === undefined) continue;
    const previous = runs[runs.length - 1];
    if (previous !== undefined && previous.tone === tone) previous.text += text;
    else runs.push({ text, tone });
  }
  return runs;
}

/**
 * A desktop lane's proportional spans: percentages over the window, with gaps
 * as a null tone. The percentages sum to 100 (within floating error).
 */
export function laneSpans(
  segments: readonly LaneSegment[],
  window: LaneWindow,
  now: number,
): LaneSpan[] {
  const duration = window.to - window.from;
  if (!(duration > 0)) return [];

  const spans = clipSpans(segments, window, now);
  const pieces: { tone: Tone | null; ms: number }[] = [];
  let cursor = window.from;
  for (const span of spans) {
    if (span.from > cursor) pieces.push({ tone: null, ms: span.from - cursor });
    const start = Math.max(span.from, cursor);
    if (span.to > start) pieces.push({ tone: span.tone, ms: span.to - start });
    cursor = Math.max(cursor, span.to);
  }
  if (cursor < window.to) pieces.push({ tone: null, ms: window.to - cursor });

  if (pieces.length === 0) return [{ tone: null, pct: 100 }];

  const merged: { tone: Tone | null; ms: number }[] = [];
  for (const piece of pieces) {
    if (piece.ms <= 0) continue;
    const previous = merged[merged.length - 1];
    if (previous !== undefined && previous.tone === piece.tone) previous.ms += piece.ms;
    else merged.push(piece);
  }

  const total = merged.reduce((sum, piece) => sum + piece.ms, 0);
  const spansOut: LaneSpan[] = [];
  let used = 0;
  for (let index = 0; index < merged.length; index += 1) {
    const piece = merged[index];
    if (piece === undefined) continue;
    const pct = (piece.ms / total) * 100;
    spansOut.push({ tone: piece.tone, pct });
    used += pct;
  }
  const lastSpan = spansOut[spansOut.length - 1];
  if (lastSpan !== undefined) lastSpan.pct = 100 - (used - lastSpan.pct);
  return spansOut;
}

// ---------------------------------------------------------------------------
// Rows, labels and messages
// ---------------------------------------------------------------------------

/** The name to draw for a run; an unnamed run uses the first 8 characters of its id. */
export function runLabel(run: Run): string {
  return run.name ?? run.id.slice(0, 8);
}

/**
 * Runs in tree order, each with the prefix its depth draws (`├ `, `└ `, and
 * `│ ` for an ancestor with later siblings). Siblings keep the snapshot's
 * order. A child whose parent is missing becomes a root; a cycle is visited
 * once and then drawn as a root.
 */
export function runRows(runs: readonly Run[]): RunRow[] {
  const ids = new Set(runs.map((run) => run.id));
  const children = new Map<string, Run[]>();
  const roots: Run[] = [];
  for (const run of runs) {
    if (run.parent_run !== null && run.parent_run !== run.id && ids.has(run.parent_run)) {
      const siblings = children.get(run.parent_run);
      if (siblings === undefined) children.set(run.parent_run, [run]);
      else siblings.push(run);
    } else {
      roots.push(run);
    }
  }

  const rows: RunRow[] = [];
  const seen = new Set<string>();

  const visit = (run: Run, ancestors: readonly boolean[]): void => {
    if (seen.has(run.id)) return;
    seen.add(run.id);
    const parents = children.get(run.parent_run ?? '') ?? [];
    const siblings =
      run.parent_run !== null && run.parent_run !== run.id && ids.has(run.parent_run)
        ? parents
        : roots;
    const index = siblings.indexOf(run);
    const isLast = index === siblings.length - 1;
    const prefix =
      ancestors.map((hasLater) => (hasLater ? '│ ' : '  ')).join('') +
      (isLast ? '└ ' : '├ ');
    rows.push({ run, label: runLabel(run), prefix, depth: ancestors.length + 1 });
    const nextAncestors = [...ancestors, !isLast];
    for (const child of children.get(run.id) ?? []) visit(child, nextAncestors);
  };

  for (const root of roots) visit(root, []);
  for (const run of runs) visit(run, []);
  return rows;
}

/** `45s`, `4m`, `1h06m`; negative and non-finite become `0s`. */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0s';
  const total = Math.floor(ms / 1000);
  if (total < 60) return `${total}s`;
  if (total < 3600) return `${Math.floor(total / 60)}m`;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return `${hours}h${String(minutes).padStart(2, '0')}m`;
}

/**
 * The axis line: `-<window>m` at the left edge, the midpoint label centered and
 * `now` at the right edge, exactly `width` columns wide. A label that would
 * overlap is left out.
 */
export function axisLabels(windowMin: number, width: number): string {
  const columns = Math.max(0, Math.floor(width));
  if (columns === 0) return '';
  const minutes = Number.isFinite(windowMin) && windowMin > 0 ? Math.round(windowMin) : 0;
  const left = `-${minutes}m`;
  const middle = `-${Math.round(minutes / 2)}m`;
  const right = 'now';
  const out: string[] = new Array<string>(columns).fill(' ');

  const place = (text: string, at: number): void => {
    if (at < 0 || at + text.length > columns) return;
    for (let index = at; index < at + text.length; index += 1) {
      if (out[index] !== ' ') return;
    }
    for (let index = 0; index < text.length; index += 1) {
      out[at + index] = text[index] ?? ' ';
    }
  };

  place(left, 0);
  place(right, columns - right.length);
  place(middle, Math.max(0, Math.floor((columns - middle.length) / 2)));
  return out.join('');
}

function lowestPool(rows: readonly QuotaRow[]): { label: string; pct: number } | null {
  let found: { label: string; pct: number } | null = null;
  for (const row of rows) {
    if (row.kind !== 'pool') continue;
    for (const pct of [row.fiveHour.pct, row.week.pct]) {
      if (pct === null || pct >= 15) continue;
      if (found === null || pct < found.pct) found = { label: row.label, pct };
    }
  }
  return found;
}

/**
 * The line above the prompt, or null when nothing needs attention: review,
 * the person, stuck and failed counts, then the single lowest pool under 15 %.
 */
export function bandText(
  counts: AttentionCounts,
  quota: readonly QuotaRow[],
): string | null {
  const parts: string[] = [];
  if (counts.claude > 0) parts.push(`◆ ${counts.claude} review`);
  if (counts.user > 0) parts.push(`● ${counts.user} you`);
  if (counts.stuck > 0) parts.push(`⚠ ${counts.stuck} stuck`);
  if (counts.failed > 0) parts.push(`✗ ${counts.failed} failed`);
  const low = lowestPool(quota);
  if (low !== null) parts.push(`· low: ${low.label} ${low.pct}%`);
  return parts.length === 0 ? null : `SO  ${parts.join('  ')}`;
}

/**
 * The status line: `SO ▶2 ◆1 ⚠1` with only non-zero counts. `SO down` while
 * the supervisor is down and the session still has active runs; null when the
 * session has no runs in the window.
 */
export function statusText(
  counts: AttentionCounts,
  supervisorRunning: boolean,
  hasActiveRuns: boolean,
  hasRuns: boolean,
): string | null {
  if (!hasRuns) return null;
  if (!supervisorRunning && hasActiveRuns) return 'SO down';
  let text = 'SO';
  if (counts.running > 0) text += ` ▶${counts.running}`;
  if (counts.claude > 0) text += ` ◆${counts.claude}`;
  if (counts.user > 0) text += ` ●${counts.user}`;
  if (counts.stuck > 0) text += ` ⚠${counts.stuck}`;
  if (counts.failed > 0) text += ` ✗${counts.failed}`;
  return text;
}

/**
 * Toasts for what became new since the previous poll: one per run that newly
 * gets attention, then `SO: Claude needs you` when the orchestrator lane newly
 * turns `user`. The first poll (null) says nothing.
 */
export function newAttention(prev: AttentionView | null, next: AttentionView): string[] {
  if (prev === null) return [];
  const before = new Map<string, Attention>();
  for (const run of prev.snapshot.runs) before.set(run.id, run.attention);

  const messages: string[] = [];
  for (const run of next.snapshot.runs) {
    if (run.attention === null) continue;
    const was = before.get(run.id) ?? null;
    if (run.attention === was) continue;
    if (run.attention === 'claude') messages.push(`SO: ${runLabel(run)} needs review`);
    else if (run.attention === 'stuck') messages.push(`SO: ${runLabel(run)} is stuck`);
    else if (run.attention === 'failed') messages.push(`SO: ${runLabel(run)} failed`);
  }

  const previousTone = openTone(prev.orchestrator, Date.parse(prev.snapshot.now));
  const nextTone = openTone(next.orchestrator, Date.parse(next.snapshot.now));
  if (nextTone === 'user' && previousTone !== 'user') {
    messages.push('SO: Claude needs you');
  }
  return messages;
}
