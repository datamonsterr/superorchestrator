import type { Elements, PluginState, RenderSurface } from 'claude-code';
import type { Account, LaneSegment, QuotaCell, Tone } from '../types';
import {
  TONES, attentionCounts, axisLabels, bandText, barCells, formatDuration,
  labelWidth, laneRuns, laneSpans, quotaRows, runLabel, runRows,
  supervisorTone, toneOfRunState, trackColor,
} from './model';

type State = PluginState['superorchestrator'];
type UI = Pick<Elements['terminal'], 'Box' | 'Text' | 'Button'>;
type Actions = { toggle: (id: string) => Promise<void>; cycle: () => Promise<void> };

// Pack worst-first quota rows into the inline pane's one or two lines.
function inlineQuota(accounts: Account[],
  columns: number, maxLines: number): string[] {
  const total = quotaRows(accounts, Infinity).length;
  if (total === 0) return ['quota  —'];
  for (let limit = Math.min(total, 4); limit > 0; limit--) {
    const cells = quotaRows(accounts, limit).map(row => row.kind === 'pool'
      ? `${row.label} ${row.fiveHour.text}/${row.week.text}${row.note ? ` ${row.note}` : ''}`
      : row.kind === 'folded' ? row.label.replace(/ ok.*/, ' ok') : `${row.label} ${row.note ?? ''}`);
    const lines = ['quota'];
    for (const cell of cells) {
      const last = lines.length - 1;
      const text = cell.slice(0, Math.max(1, columns - 7));
      if ((lines[last]!.length + 2 + text.length) <= columns) lines[last] += `  ${text}`;
      else lines.push(`       ${text}`);
    }
    if (lines.length <= maxLines) return lines;
  }
  return ['quota'];
}

export function renderPane(ui: UI, state: State, surface: RenderSurface,
  columns: number, rows: number, now: number, placement: 'dock' | 'inline', actions: Actions) {
  const { Box, Text, Button } = ui;
  const snapshot = state.snapshot;
  const track = trackColor(state.theme);
  const labelCols = labelWidth(columns);
  const barWidth = Math.max(1, columns - labelCols - 6);
  const window = { from: now - state.windowMin * 60_000, to: now };
  const terminal = surface === 'terminal';
  const mobile = surface === 'mobile';
  const inline = placement === 'inline';
  const caption = (text: string) => <Text mono size="caption">{text}</Text>;
  const lane = (segments: LaneSegment[]) => terminal
    ? <Text mono>{laneRuns(segments, window, barWidth, now).map(part => {
        const tone = TONES[part.tone];
        return <Text color={part.tone === 'idle' || part.tone === 'queued' ? track.color : tone.theme}
          dimColor={part.tone === 'idle' || part.tone === 'queued' ? track.dim : tone.dim}>{part.text}</Text>;
      })}</Text>
    : <Box flexDirection="row" flexGrow={1} height={1} backgroundColor={track.color}>
        {/* This build accepts whole-number percentage widths. */}
        {laneSpans(segments, window, now).map(span => <Box width={`${Math.round(Math.max(0, span.pct))}%`}
          height={1} backgroundColor={span.tone === null ? track.color : TONES[span.tone].theme} />)}
      </Box>;
  const quotaCell = (cell: QuotaCell) => {
    if (mobile) return <Text mono size="caption" color={cell.tone}>{cell.text} </Text>;
    const bar = barCells(cell.pct ?? 0, Math.max(1, Math.floor((columns - 25) / 2)));
    if (terminal) return <Text><Text color={cell.tone}>{'■'.repeat(bar.filled)}</Text>
      <Text color={track.color} dimColor={track.dim}>{'■'.repeat(bar.empty)}</Text>
      <Text color={cell.tone}> {cell.text} </Text></Text>;
    return <Box flexDirection="row" flexGrow={1}>
      <Box flexDirection="row" flexGrow={1} height={1} backgroundColor={track.color}>
        <Box width={`${Math.round(cell.pct ?? 0)}%`} height={1} backgroundColor={cell.tone} />
      </Box><Text mono size="caption" color={cell.tone}> {cell.text} </Text>
    </Box>;
  };
  const error = state.fetch.status === 'missing'
    ? 'superorchestrator is not on PATH'
    : state.fetch.status === 'error'
      ? snapshot === null ? 'Monitor poll failed.' : 'Monitor poll failed; showing last good data.'
      : null;
  if (snapshot === null) return <Box flexDirection="column">
    <Text weight="bold">Superorchestrator</Text>
    <Text color={error ? 'error' : 'inactive'}>{error ?? 'Loading monitor…'}</Text>
  </Box>;

  const health = supervisorTone(snapshot.supervisor);
  const counts = attentionCounts(snapshot, state.orchestrator, now);
  const countParts: { count: number; tone: Tone; label: string }[] = [
    { count: counts.running, tone: 'running', label: 'run' },
    { count: counts.claude, tone: 'claude', label: 'Claude' },
    { count: counts.user, tone: 'user', label: 'you' },
    { count: counts.stuck, tone: 'stuck', label: '' },
    { count: counts.failed, tone: 'failed', label: '' },
  ];
  const allRuns = runRows(snapshot.runs);
  const quotaBudget = rows < allRuns.length + 14 ? 1 : Math.min(5, Math.max(1, rows - allRuns.length - 9));
  const quota = quotaRows(snapshot.accounts, quotaBudget);
  const packed = inline ? inlineQuota(snapshot.accounts, columns, rows < 18 ? 1 : 2) : [];
  const fixedRows = (mobile ? 4 : 7) + (inline ? packed.length : quota.length + 1) + (error ? 1 : 0);
  const runBudget = Math.max(0, rows - fixedRows);
  // Select attention first, then restore tree order among the retained rows.
  const needsAttention = (run: typeof snapshot.runs[number]) => run.attention !== null
    || ['review', 'stuck', 'failed'].includes(toneOfRunState(run.state, run.question !== null));
  const ranked = [...allRuns].sort((a, b) => Number(needsAttention(b.run)) - Number(needsAttention(a.run)));
  const selected = new Set<string>();
  let used = 0;
  for (const row of ranked) {
    const cost = 1 + Number(state.expanded.includes(row.run.id));
    if (used + cost > runBudget - Number(selected.size < allRuns.length - 1)) continue;
    selected.add(row.run.id); used += cost;
  }
  return <Box flexDirection="column">
    <Text weight="bold">Superorchestrator <Text color={TONES[health].theme}>
      {health === 'down' ? 'supervisor down' : health === 'stale'
        ? `⚠ ${snapshot.supervisor.warning ?? 'stale build'}`
        : `● up ${formatDuration((snapshot.supervisor.uptime_s ?? 0) * 1000)}`}
    </Text></Text>
    {error && <Text color="error">{error}</Text>}
    {!inline && <Text mono size="caption">quota          5h        week</Text>}
    {inline && packed.map(line => <Text mono size="caption">{line}</Text>)}
    {!inline && quota.map(row => <Box flexDirection="row">
      <Text mono size="caption">{row.label} </Text>
      {row.kind === 'pool' ? <Box flexDirection="row" flexGrow={1}>
        {quotaCell(row.fiveHour)}{quotaCell(row.week)}
        {row.note && <Text color={row.noteTone ?? 'inactive'}>{row.note}</Text>}
      </Box> : <Text color={row.noteTone ?? 'inactive'}>{row.note}</Text>}
    </Box>)}
    <Text mono size="caption">{countParts.filter(p => p.count > 0).map(p =>
      <Text color={TONES[p.tone].theme}>{TONES[p.tone].mark}{p.count}{p.label ? ` ${p.label}` : ''}  </Text>)}</Text>
    {!mobile && <Box flexDirection="row">{caption(' '.repeat(labelCols))}{caption(axisLabels(state.windowMin, barWidth))}</Box>}
    {!mobile && <Box flexDirection="row">{caption('supervisor'.slice(0, labelCols).padEnd(labelCols))}{lane(state.supervisor)}</Box>}
    {!mobile && <Box flexDirection="row">{caption('claude'.padEnd(labelCols))}{lane(state.orchestrator)}</Box>}
    {snapshot.session === null && <Text>No superorchestrator runs in this conversation yet.</Text>}
    {allRuns.filter(row => selected.has(row.run.id)).map(({ run, prefix }) => {
      const tone = TONES[toneOfRunState(run.state, run.question !== null)];
      const label = `${prefix}${runLabel(run)}`.slice(0, labelCols).padEnd(labelCols);
      const duration = formatDuration((run.finished_at === null ? now : Date.parse(run.finished_at))
        - Date.parse(run.started_at ?? run.created_at));
      const segments = run.segments.map(s => ({ tone: toneOfRunState(s.state,
        s.to === null && run.question !== null), from: Date.parse(s.from),
        to: s.to === null ? null : Date.parse(s.to) }));
      return <Box flexDirection="column">
        <Box flexDirection="row">
          <Button key={run.id} plain onPress={() => actions.toggle(run.id)}>
            <Text mono size="caption">{mobile ? `${prefix}${runLabel(run)} ` : label}</Text>
            {terminal && lane(segments)}
            {(terminal || mobile) && <Text color={tone.theme}>{tone.mark}{duration.padStart(5)}</Text>}
          </Button>
          {!terminal && !mobile && lane(segments)}
          {!terminal && !mobile && <Text mono size="caption" color={tone.theme}>{tone.mark}{duration.padStart(5)}</Text>}
        </Box>
        {state.expanded.includes(run.id) && <Text mono size="caption" dimColor>
          {[run.model, run.account, ('branch' in run && typeof run.branch === 'string') ? run.branch : null, run.question].filter(Boolean).join(' · ')}
        </Text>}
      </Box>;
    })}
    {selected.size < allRuns.length && <Text dimColor>+{allRuns.length - selected.size} more</Text>}
    <Button key="window" plain hotkey="w" label={`Window ${state.windowMin}m`} onPress={actions.cycle} />
  </Box>;
}

export function renderBand(ui: UI, state: State, now: number) {
  if (state.snapshot === null) return null;
  const { Text } = ui;
  const text = bandText(attentionCounts(state.snapshot, state.orchestrator, now),
    quotaRows(state.snapshot.accounts, Infinity));
  if (text === null) return null;
  return <Text mono size="caption">{text.split('  ').map(part => {
    const color = part.startsWith('◆') ? TONES.claude.theme
      : part.startsWith('●') ? TONES.user.theme
      : part.startsWith('✗') ? TONES.failed.theme
      : part.startsWith('⚠') || part.startsWith('· low:') ? TONES.stuck.theme : 'inactive';
    return <Text color={color}>{part}  </Text>;
  })}</Text>;
}
