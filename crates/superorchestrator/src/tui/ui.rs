//! Drawing: a pure function of [`App`] and the terminal size.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Padding, Paragraph, Row, Table, Tabs, Wrap};
use so_accounts::SignIn;
use so_core::RunState;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app::{
    AddField, App, DetailView, HARNESSES, Modal, SetupStep, StepState, Tab, ToastKind,
};
use super::data::{Check, Level, RunItem, Snapshot};
use super::theme::Theme;

/// Below this the layout cannot hold its content; a message is shown instead.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

pub fn draw(f: &mut Frame, app: &mut App) {
    let full = f.area();
    if full.width < MIN_WIDTH || full.height < MIN_HEIGHT {
        let msg = format!(
            "Terminal too small ({}x{}).\nMake it at least {MIN_WIDTH}x{MIN_HEIGHT}.",
            full.width, full.height
        );
        f.render_widget(
            Paragraph::new(msg)
                .alignment(Alignment::Center)
                .style(app.theme.warn()),
            full,
        );
        return;
    }
    let area = full.inner(Margin::new(1, 0));
    let [header, tabs, body, status, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, app, header);
    draw_tabs(f, app, tabs);
    match app.tab {
        Tab::Overview => draw_overview(f, app, body),
        Tab::Accounts => draw_accounts(f, app, body),
        Tab::Runs => draw_runs(f, app, body),
        Tab::Setup => draw_setup(f, app, body),
    }
    draw_status(f, app, status);
    draw_hints(f, app, hints);
    if let Some(m) = app.modal.clone() {
        draw_modal(f, app, &m, full);
    }
}

// ------------------------------------------------------------------- text helpers

/// Cuts `s` to `max` display columns, ending in an ellipsis when it was cut.
pub fn truncate(s: &str, max: usize, ellipsis: &str) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(ellipsis.width());
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > room {
            break;
        }
        out.push(c);
        w += cw;
    }
    if max >= ellipsis.width() {
        out.push_str(ellipsis);
    }
    out
}

/// Like [`truncate`] but keeps the end (paths: the last part matters).
fn truncate_start(s: &str, max: usize, ellipsis: &str) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(ellipsis.width());
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw > room {
            break;
        }
        tail.push(c);
        w += cw;
    }
    tail.reverse();
    format!("{ellipsis}{}", tail.into_iter().collect::<String>())
}

/// "5s", "3m", "2h", "4d".
pub fn age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let s = (now - then).num_seconds().max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn block<'a>(theme: &Theme, title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_set(theme.border_set())
        .border_style(theme.muted())
        .title(title)
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn placeholder(f: &mut Frame, theme: &Theme, area: Rect, lines: Vec<Line<'static>>) {
    let h = lines.len() as u16;
    let r = centered(area.width, h, area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);
    let _ = theme;
}

// ---------------------------------------------------------------------- chrome

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut spans = vec![
        Span::styled("superorchestrator", t.accent()),
        Span::styled(format!(" {}", env!("CARGO_PKG_VERSION")), t.muted()),
        Span::raw("  "),
    ];
    match app.snap.as_ref().and_then(|s| s.supervisor.as_ref()) {
        Some(s) => {
            spans.push(Span::styled(format!("{} ", t.dot()), t.ok()));
            spans.push(Span::raw(format!("supervisor up {}", duration(s.uptime_s))));
        }
        None if app.snap.is_some() => {
            spans.push(Span::styled(format!("{} ", t.circle()), t.muted()));
            spans.push(Span::styled("supervisor not running", t.muted()));
        }
        None => spans.push(Span::styled("supervisor ...", t.muted())),
    }
    let left: usize = spans.iter().map(Span::width).sum();
    let room = (area.width as usize).saturating_sub(left + 2);
    if let Some(s) = &app.snap
        && room >= 8
    {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            truncate_start(&s.config_dir, room, t.ellipsis()),
            t.muted(),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn duration(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|tab| {
            let mut label = format!(" {} {} ", tab.index() + 1, tab.title());
            if let Some(s) = &app.snap {
                match tab {
                    Tab::Runs if s.pending_review > 0 => {
                        label = format!(
                            " {} {} {}{} ",
                            tab.index() + 1,
                            tab.title(),
                            t.bang(),
                            s.pending_review
                        );
                    }
                    Tab::Runs if s.active_runs > 0 => {
                        label = format!(" {} {} {} ", tab.index() + 1, tab.title(), s.active_runs);
                    }
                    _ => {}
                }
            }
            Line::from(label)
        })
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .style(t.muted())
            .highlight_style(t.selected())
            .divider(" "),
        area,
    );
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let line = if let Some(label) = app.busy {
        Line::from(vec![
            Span::styled(format!("{} ", t.spinner(app.tick)), t.accent()),
            Span::raw(format!("{label}{}", t.ellipsis())),
        ])
    } else if let Some(toast) = &app.toast {
        let (sym, style) = match toast.kind {
            ToastKind::Ok => (t.tick(), t.ok()),
            ToastKind::Err => (t.cross(), t.bad()),
        };
        let text = truncate(
            &toast.text,
            (area.width as usize).saturating_sub(2),
            t.ellipsis(),
        );
        Line::from(vec![
            Span::styled(format!("{sym} "), style),
            Span::styled(text, style),
        ])
    } else if let Some(s) = &app.snap {
        Line::styled(format!("updated {} ago", age(app.now, s.at)), t.muted())
    } else {
        Line::styled(format!("loading{}", t.ellipsis()), t.muted())
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Key hints for the view; the last items are dropped first when the line is too long.
fn hints_for(app: &App) -> (Vec<(&'static str, &'static str)>, &'static str) {
    if app.modal.is_some() {
        return (vec![("Esc", "close")], "");
    }
    let nav = ("1-4", "tabs");
    match app.tab {
        Tab::Overview => (vec![nav, ("R", "refresh")], "quit"),
        Tab::Accounts => (
            vec![
                ("j/k", "move"),
                ("a", "add"),
                ("l", "sign in"),
                ("e", "enable"),
                ("f", "flag"),
                ("x", "remove"),
                ("i", "scan"),
            ],
            "quit",
        ),
        Tab::Runs if app.detail.is_some() => (
            vec![
                ("j/k", "scroll"),
                ("Esc", "back"),
                ("o", "join"),
                ("a", "accept"),
                ("r", "reject"),
                ("c", "cancel"),
            ],
            "back",
        ),
        Tab::Runs => (
            vec![
                ("j/k", "move"),
                ("Enter", "detail"),
                ("o", "join"),
                ("a", "accept"),
                ("r", "reject"),
                ("c", "cancel"),
            ],
            "quit",
        ),
        Tab::Setup => (
            vec![
                ("j/k", "move"),
                ("Enter", "run step"),
                ("a", "run all"),
                ("i", "install"),
                ("d", "doctor"),
            ],
            "quit",
        ),
    }
}

fn draw_hints(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let (mut items, q) = hints_for(app);
    let tail: &[(&str, &str)] = if q.is_empty() {
        &[]
    } else {
        &[("?", "help"), ("q", q)]
    };
    let width = |items: &[(&str, &str)]| -> usize {
        items
            .iter()
            .chain(tail.iter())
            .map(|(k, d)| k.width() + 1 + d.width())
            .sum::<usize>()
            + 2 * (items.len() + tail.len()).saturating_sub(1)
    };
    while items.len() > 1 && width(&items) > area.width as usize {
        items.pop();
    }
    let mut spans = Vec::new();
    for (i, (k, d)) in items.iter().chain(tail.iter()).enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*k, t.bold()));
        spans.push(Span::styled(format!(" {d}"), t.muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// -------------------------------------------------------------------- overview

fn level_mark(t: &Theme, l: Level) -> (&'static str, Style) {
    match l {
        Level::Ok => (t.tick(), t.ok()),
        Level::Warn => (t.bang(), t.warn()),
        Level::Missing => (t.cross(), t.bad()),
    }
}

fn card(f: &mut Frame, t: &Theme, area: Rect, label: &str, value: String, sub: String, v: Style) {
    let b = block(t, Span::styled(format!(" {label} "), t.muted()));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let w = inner.width as usize;
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(truncate(&value, w, t.ellipsis()), v),
            Line::styled(truncate(&sub, w, t.ellipsis()), t.muted()),
        ])
        .block(Block::new().padding(Padding::horizontal(1))),
        inner,
    );
}

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some(s) = &app.snap else {
        return loading(f, app, area);
    };
    let done = s.checks.iter().filter(|c| c.level == Level::Ok).count();
    let fixes = s.checks.iter().filter(|c| c.fix.is_some()).count();
    let check_rows = (1 + s.checks.len() + fixes) as u16;
    let [cards, list, feed] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(check_rows + 1),
        Constraint::Min(0),
    ])
    .areas(area);

    let cols: [Rect; 4] = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .areas(cards);
    let accounts_ok = !s.accounts.is_empty();
    card(
        f,
        t,
        cols[0],
        "Accounts",
        s.accounts.len().to_string(),
        format!("{} signed in", s.signed_in()),
        if accounts_ok { t.accent() } else { t.warn() },
    );
    card(
        f,
        t,
        cols[1],
        "Active runs",
        s.active_runs.to_string(),
        if s.supervisor.is_some() {
            "supervisor up".to_string()
        } else {
            "supervisor down".to_string()
        },
        t.accent(),
    );
    card(
        f,
        t,
        cols[2],
        "To review",
        s.pending_review.to_string(),
        if s.pending_review > 0 {
            "open Runs (3)".to_string()
        } else {
            "nothing waiting".to_string()
        },
        if s.pending_review > 0 {
            t.warn()
        } else {
            t.accent()
        },
    );
    card(
        f,
        t,
        cols[3],
        "Setup",
        format!("{done} of {}", s.checks.len()),
        if done == s.checks.len() {
            "all set".to_string()
        } else {
            "see below".to_string()
        },
        if done == s.checks.len() {
            t.ok()
        } else {
            t.warn()
        },
    );

    let list = list.inner(Margin::new(1, 0));
    let mut lines = vec![Line::styled("Setup checklist", t.bold())];
    for c in &s.checks {
        lines.extend(check_lines(t, c, list.width as usize));
    }
    f.render_widget(Paragraph::new(lines), list);

    if feed.height >= 3 && !s.runs.is_empty() {
        let feed = feed.inner(Margin::new(1, 0));
        let mut lines = vec![Line::styled("Recent runs", t.bold())];
        for r in s.runs.iter().take(feed.height as usize - 1) {
            lines.push(run_line(t, r, app.now, feed.width as usize));
        }
        f.render_widget(Paragraph::new(lines), feed);
    }
}

const LABEL_W: usize = 13;

fn check_lines(t: &Theme, c: &Check, width: usize) -> Vec<Line<'static>> {
    let (mark, style) = level_mark(t, c.level);
    let detail_w = width.saturating_sub(LABEL_W + 3);
    let mut out = vec![Line::from(vec![
        Span::styled(format!("{mark} "), style),
        Span::raw(format!("{:<LABEL_W$} ", c.label)),
        Span::raw(truncate(&c.detail, detail_w, t.ellipsis())),
    ])];
    if let Some(fix) = &c.fix {
        out.push(Line::from(vec![
            Span::raw(" ".repeat(LABEL_W + 3)),
            Span::styled(
                truncate(&format!("{} {fix}", t.arrow()), detail_w, t.ellipsis()),
                t.muted(),
            ),
        ]));
    }
    out
}

fn loading(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    placeholder(
        f,
        t,
        area,
        vec![Line::styled(
            format!("{} loading{}", t.spinner(app.tick), t.ellipsis()),
            t.muted(),
        )],
    );
}

// -------------------------------------------------------------------- accounts

fn signed_cell(t: &Theme, s: SignIn) -> Cell<'static> {
    match s {
        SignIn::SignedIn => Cell::new(format!("{} yes", t.tick())).style(t.ok()),
        SignIn::SignedOut => Cell::new(format!("{} no", t.cross())).style(t.bad()),
        SignIn::Unknown => Cell::new("? unknown").style(t.muted()),
    }
}

fn draw_accounts(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let Some(s) = &app.snap else {
        return loading(f, app, area);
    };
    if let Some(e) = &s.accounts_error {
        let msg = format!("{} could not read accounts: {e}", t.cross());
        return placeholder(f, &t, area, vec![Line::styled(msg, t.bad())]);
    }
    if s.accounts.is_empty() {
        return placeholder(
            f,
            &t,
            area,
            vec![
                Line::styled("No accounts yet", t.bold()),
                Line::raw(""),
                Line::raw(
                    "Press a to add accounts, or i to import logins already on this machine.",
                ),
                Line::styled("Setup (4) does both in one go.", t.muted()),
            ],
        );
    }
    // Below 90 columns ENABLED and CAP go, so the headroom text keeps its room; a disabled
    // account is then marked "(off)" after its name.
    let wide = area.width >= 90;
    let widths: Vec<Constraint> = if wide {
        vec![
            Constraint::Length(18),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Fill(1),
        ]
    } else {
        vec![
            Constraint::Length(18),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Fill(1),
        ]
    };
    let inner = area.inner(Margin::new(1, 0));
    let cols = Layout::horizontal(widths.clone())
        .spacing(1)
        .split(Rect::new(
            inner.x + 2,
            inner.y,
            inner.width.saturating_sub(2),
            1,
        ));
    let w = |i: usize| cols[i].width as usize;
    let last = cols.len() - 1;
    let rows: Vec<Row> = s
        .accounts
        .iter()
        .map(|a| {
            let dim = if a.enabled { t.base() } else { t.muted() };
            let name = if wide || a.enabled {
                a.name.clone()
            } else {
                format!("{} (off)", a.name)
            };
            let mut cells = vec![
                Cell::new(truncate(&name, w(0), t.ellipsis())),
                Cell::new(a.harness.as_str()),
                signed_cell(&t, a.signed_in),
            ];
            if wide {
                cells.push(Cell::new(if a.enabled { "yes" } else { "no" }));
                cells.push(Cell::new(a.cap.to_string()));
            }
            cells.push(Cell::new(a.active.to_string()));
            cells.push(Cell::new(truncate(&a.headroom, w(last), t.ellipsis())));
            Row::new(cells).style(dim)
        })
        .collect();
    let header = Row::new(if wide {
        vec![
            "NAME",
            "HARNESS",
            "SIGNED IN",
            "ENABLED",
            "CAP",
            "ACTIVE",
            "HEADROOM",
        ]
    } else {
        vec!["NAME", "HARNESS", "SIGNED IN", "ACTIVE", "HEADROOM"]
    })
    .style(t.header());
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .highlight_symbol(format!("{} ", t.pointer()))
        .row_highlight_style(t.selected());
    app.accounts_state.select(Some(app.accounts_sel));
    f.render_stateful_widget(table, inner, &mut app.accounts_state);
}

// ------------------------------------------------------------------------ runs

fn state_mark(t: &Theme, s: RunState) -> (&'static str, Style) {
    match s {
        RunState::Running => (t.dot(), t.accent()),
        RunState::Queued | RunState::Starting => (t.circle(), t.muted()),
        RunState::Waiting | RunState::Stalled | RunState::Escalated | RunState::PendingReview => {
            (t.bang(), t.warn())
        }
        RunState::Succeeded => (t.tick(), t.ok()),
        RunState::Failed => (t.cross(), t.bad()),
        RunState::Cancelled => ("-", t.muted()),
    }
}

fn who(r: &RunItem) -> String {
    match &r.account {
        Some(a) => a.clone(),
        None => r.harness.as_str().to_string(),
    }
}

fn run_line(t: &Theme, r: &RunItem, now: DateTime<Utc>, width: usize) -> Line<'static> {
    let (mark, style) = state_mark(t, r.state);
    let state = r.state.to_string();
    let head = format!("{:<14} {:<18} ", state, truncate(&who(r), 18, t.ellipsis()));
    let age = format!("{:<4}", age(now, r.created_at));
    let room = width.saturating_sub(2 + head.width() + age.width() + 1);
    Line::from(vec![
        Span::styled(format!("{mark} "), style),
        Span::styled(head, style),
        Span::raw(format!("{age} ")),
        Span::styled(truncate(&r.title, room, t.ellipsis()), t.muted()),
    ])
}

fn draw_runs(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    if app.snap.is_none() {
        return loading(f, app, area);
    }
    if app.detail.is_some() {
        return draw_run_detail(f, app, area);
    }
    let Some(s) = &app.snap else { return };
    if s.runs.is_empty() {
        let mut lines = vec![
            Line::styled("No runs yet", t.bold()),
            Line::raw(""),
            Line::raw("Runs appear here as soon as an orchestrator delegates work."),
        ];
        if s.supervisor.is_none() {
            lines.push(Line::styled(
                "The supervisor is not running; it starts with the first delegation.",
                t.muted(),
            ));
        }
        return placeholder(f, &t, area, lines);
    }
    let widths = [
        Constraint::Length(8),
        Constraint::Length(16),
        Constraint::Length(18),
        Constraint::Length(5),
        Constraint::Fill(1),
    ];
    let inner = area.inner(Margin::new(1, 0));
    let cols = Layout::horizontal(widths).spacing(1).split(Rect::new(
        inner.x + 2,
        inner.y,
        inner.width.saturating_sub(2),
        1,
    ));
    let rows: Vec<Row> = s
        .runs
        .iter()
        .map(|r| {
            let (mark, style) = state_mark(&t, r.state);
            Row::new(vec![
                Cell::new(short(&r.id)),
                Cell::new(format!("{mark} {}", r.state)).style(style),
                Cell::new(truncate(&who(r), cols[2].width as usize, t.ellipsis())),
                Cell::new(age(app.now, r.created_at)),
                Cell::new(truncate(&r.title, cols[4].width as usize, t.ellipsis())),
            ])
        })
        .collect();
    let header = Row::new(["RUN", "STATE", "ACCOUNT", "AGE", "TITLE"]).style(t.header());
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .highlight_symbol(format!("{} ", t.pointer()))
        .row_highlight_style(t.selected());
    app.runs_state.select(Some(app.runs_sel));
    f.render_stateful_widget(table, inner, &mut app.runs_state);
}

const BRIEF_LINES: usize = 10;

fn detail_lines(
    t: &Theme,
    d: &DetailView,
    now: DateTime<Utc>,
    snap: &Snapshot,
) -> Vec<Line<'static>> {
    let heading = |s: &str| Line::styled(s.to_string(), t.bold());
    let kv = |k: &str, v: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<10}"), t.muted()),
            Span::raw(v.to_string()),
        ])
    };
    let Some(data) = &d.data else {
        return vec![Line::styled(format!("loading{}", t.ellipsis()), t.muted())];
    };
    let r = match data {
        Ok(r) => r,
        Err(e) => return vec![Line::styled(format!("{} {e}", t.cross()), t.bad())],
    };
    let created = snap
        .runs
        .iter()
        .find(|x| x.id == r.id)
        .map(|x| age(now, x.created_at));
    let (mark, style) = state_mark(t, r.state);
    let mut out = vec![
        Line::from(vec![
            Span::styled(format!("{mark} {}", r.state), style),
            Span::styled(
                created
                    .map(|a| format!("  started {a} ago"))
                    .unwrap_or_default(),
                t.muted(),
            ),
        ]),
        kv(
            "runs on",
            &format!(
                "{} / {} / {}",
                r.harness,
                r.account.as_deref().unwrap_or("-"),
                r.model.as_deref().unwrap_or("-")
            ),
        ),
    ];
    if let Some(w) = &r.worktree {
        out.push(kv("worktree", w));
    }
    if let Some(b) = &r.branch {
        out.push(kv("branch", b));
    }
    if let Some(e) = &r.error {
        out.push(Line::from(vec![
            Span::styled(format!("{:<10}", "error"), t.muted()),
            Span::styled(e.clone(), t.bad()),
        ]));
    }
    out.push(Line::raw(""));
    out.push(heading("Brief"));
    let brief: Vec<&str> = r.brief.lines().collect();
    if brief.is_empty() {
        out.push(Line::styled("(none recorded)", t.muted()));
    }
    out.extend(
        brief
            .iter()
            .take(BRIEF_LINES)
            .map(|l| Line::raw(l.to_string())),
    );
    if brief.len() > BRIEF_LINES {
        out.push(Line::styled(
            format!("{} {} more lines", t.ellipsis(), brief.len() - BRIEF_LINES),
            t.muted(),
        ));
    }
    out.push(Line::raw(""));
    out.push(heading("Route"));
    out.extend(r.route.iter().map(|l| Line::raw(l.clone())));
    out.push(Line::raw(""));
    out.push(heading("Recent events"));
    if r.events.is_empty() {
        out.push(Line::styled("(none yet)", t.muted()));
    }
    out.extend(r.events.iter().map(|l| Line::raw(l.clone())));
    out.push(Line::raw(""));
    out.push(heading("Result"));
    match &r.result {
        Some(res) => out.extend(res.lines().map(|l| Line::raw(l.to_string()))),
        None => out.push(Line::styled("(not finished)", t.muted())),
    }
    match &d.diff {
        None => {}
        Some(Err(e)) => {
            out.push(Line::raw(""));
            out.push(heading("Diff"));
            out.push(Line::styled(format!("{} {e}", t.bang()), t.warn()));
        }
        Some(Ok(d)) => {
            out.push(Line::raw(""));
            out.push(heading(&format!(
                "Diff  {} (from {})",
                d.branch,
                short(&d.base)
            )));
            out.extend(clean_diff(&d.stat).map(Line::raw));
            out.push(Line::raw(""));
            for l in clean_diff(&d.patch) {
                let style = match l.chars().next() {
                    _ if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") => {
                        t.bold()
                    }
                    Some('+') => t.ok(),
                    Some('-') => t.bad(),
                    Some('@') => t.accent(),
                    _ => t.base(),
                };
                out.push(Line::styled(l, style));
            }
            if d.truncated {
                out.push(Line::styled(
                    format!("{} diff truncated", t.ellipsis()),
                    t.muted(),
                ));
            }
        }
    }
    out
}

/// Diff text one line at a time, with tabs and control characters made harmless.
fn clean_diff(s: &str) -> impl Iterator<Item = String> + '_ {
    s.lines().map(|l| {
        l.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    })
}

fn draw_run_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let now = app.now;
    let (Some(d), Some(s)) = (&mut app.detail, &app.snap) else {
        return;
    };
    let mut b = block(
        &t,
        Span::styled(format!(" run {} ", short(&d.id)), t.bold()),
    )
    .padding(Padding::horizontal(1));
    let inner = b.inner(area);
    let lines = detail_lines(&t, d, now, s);
    // Keep the scroll state inside the content (wrapped lines counted), so one key press
    // undoes one press. Word wrapping can need a line more than this estimate.
    let w = usize::from(inner.width.max(1));
    let rows: usize = lines
        .iter()
        .map(|l| l.width().div_ceil(w).max(1))
        .sum::<usize>()
        + 1;
    let max = rows.saturating_sub(usize::from(inner.height));
    d.scroll = d.scroll.min(max as u16);
    if rows > usize::from(inner.height) + usize::from(d.scroll) {
        b = b.title_bottom(
            Line::styled(format!(" {} more ", t.arrow_down()), t.muted()).right_aligned(),
        );
    }
    f.render_widget(b, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((d.scroll, 0)),
        inner,
    );
}

// ----------------------------------------------------------------------- setup

fn draw_setup(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let area = area.inner(Margin::new(1, 0));
    let [intro, list, out] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(SetupStep::ALL.len() as u16 + 1),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(vec![
            Line::raw("First-run steps. Safe to repeat; each does only what is missing."),
            Line::raw(""),
        ])
        .style(t.muted()),
        intro,
    );
    let mut lines = Vec::new();
    for (i, step) in SetupStep::ALL.iter().enumerate() {
        let row = &app.setup[i];
        let (mark, style) = match row.state {
            StepState::Pending => (t.circle().to_string(), t.muted()),
            StepState::Running => (t.spinner(app.tick).to_string(), t.accent()),
            StepState::Done => (t.tick().to_string(), t.ok()),
            StepState::Warn => (t.bang().to_string(), t.warn()),
            StepState::Failed => (t.cross().to_string(), t.bad()),
        };
        let sel = i == app.setup_sel;
        let pointer = if sel { t.pointer() } else { " " };
        let label = format!("{:<13}", step.label());
        let room = (list.width as usize).saturating_sub(2 + 2 + 13 + 1);
        let desc = truncate(step.describe(), room, t.ellipsis());
        let mut line = Line::from(vec![
            Span::raw(format!("{pointer} ")),
            Span::styled(format!("{mark} "), style),
            Span::styled(label, if sel { t.bold() } else { t.base() }),
            Span::styled(format!(" {desc}"), t.muted()),
        ]);
        if sel {
            line = line.patch_style(t.selected());
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), list);

    let step = SetupStep::ALL[app.setup_sel];
    let row = &app.setup[app.setup_sel];
    let b = block(t, Span::styled(format!(" {} ", step.label()), t.muted()))
        .padding(Padding::horizontal(1));
    let inner = b.inner(out);
    f.render_widget(b, out);
    let body: Vec<Line> = match row.state {
        StepState::Pending => vec![Line::styled(
            format!("Not run yet. Press Enter to run `{}`.", step.label()),
            t.muted(),
        )],
        StepState::Running => vec![Line::styled(format!("running{}", t.ellipsis()), t.muted())],
        _ => row.lines.iter().map(|l| Line::raw(l.clone())).collect(),
    };
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------------- modals

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let t = &app.theme;
    let row = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), t.bold()),
            Span::raw(d.to_string()),
        ])
    };
    let mut out = vec![
        Line::styled("Everywhere", t.accent()),
        row("Tab  S-Tab", "next / previous tab"),
        row("1 2 3 4", "Overview, Accounts, Runs, Setup"),
        row("R  F5", "refresh now (the screen refreshes every 2 s)"),
        row("?", "this help"),
        row("q  Esc", "close a pane, or quit at the top level"),
        row("Ctrl-C", "quit"),
        Line::raw(""),
    ];
    match app.tab {
        Tab::Overview => out.push(Line::styled(
            "Overview shows what is set up and what to do next.",
            t.muted(),
        )),
        Tab::Accounts => {
            out.push(Line::styled("Accounts", t.accent()));
            out.extend([
                row(
                    "j k  g G",
                    "move; top or bottom (arrows, PgUp, Home work too)",
                ),
                row("a", "add accounts (harness, count)"),
                row(
                    "l",
                    "sign in (claude and agy: in a session of their wrapper)",
                ),
                row("e", "enable or disable"),
                row("f", "flag a cooldown, or clear it"),
                row("x", "remove (type the name to confirm)"),
                row("i", "scan for logins already on disk and import them"),
            ]);
        }
        Tab::Runs => {
            out.push(Line::styled("Runs", t.accent()));
            out.extend([
                row("j k  g G", "move, or scroll an open run"),
                row("Enter", "open the run's details"),
                row("c", "cancel (asks first)"),
                row("o", "open the run's session (join)"),
                row("a", "accept and merge a run waiting for review"),
                row("A", "accept without merging"),
                row("r", "reject with feedback"),
            ]);
        }
        Tab::Setup => {
            out.push(Line::styled("Setup", t.accent()));
            out.extend([
                row("Enter", "run the selected step"),
                row("a", "run every step in order"),
                row("i  d", "run install or doctor"),
            ]);
        }
    }
    out
}

fn input_line(t: &Theme, typed: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("> ", t.accent()),
        Span::raw(typed.to_string()),
    ])
}

fn draw_modal(f: &mut Frame, app: &App, modal: &Modal, full: Rect) {
    let t = &app.theme;
    let muted = |s: &str| Line::styled(s.to_string(), t.muted());
    let mut cursor: Option<(u16, u16)> = None;
    let (title, lines, width): (String, Vec<Line>, u16) = match modal {
        Modal::Help => ("Help".into(), help_lines(app), 76),
        Modal::Message { title, lines } => (
            title.clone(),
            lines
                .iter()
                .map(|l| Line::raw(l.clone()))
                .chain([Line::raw(""), muted("Enter or Esc to close")])
                .collect(),
            72,
        ),
        Modal::Confirm { prompt, .. } => (
            "Confirm".into(),
            vec![
                Line::raw(prompt.clone()),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("y", t.bold()),
                    Span::styled(" yes   ", t.muted()),
                    Span::styled("n", t.bold()),
                    Span::styled(" no (default)", t.muted()),
                ]),
            ],
            56,
        ),
        Modal::TypeName { name, typed } => {
            cursor = Some((2 + typed.width() as u16, 3));
            (
                "Remove account".into(),
                vec![
                    Line::raw(format!(
                        "Removing {name} deletes its account file and wrapper."
                    )),
                    Line::raw("Type the account name to confirm:"),
                    Line::raw(""),
                    input_line(t, typed),
                    Line::raw(""),
                    muted("Enter confirm   Esc cancel"),
                ],
                64,
            )
        }
        Modal::Input { title, typed, .. } => {
            cursor = Some((2 + typed.width() as u16, 2));
            (
                "Input".into(),
                vec![
                    Line::raw(title.clone()),
                    Line::raw(""),
                    input_line(t, typed),
                    Line::raw(""),
                    muted("Enter submit   Esc cancel"),
                ],
                64,
            )
        }
        Modal::AddAccount {
            harness,
            count,
            field,
        } => {
            let hl = |on: bool| if on { t.selected() } else { t.base() };
            let mut opts = vec![Span::styled("Harness  ", t.muted())];
            for (i, h) in HARNESSES.iter().enumerate() {
                opts.push(Span::styled(format!(" {} ", h.as_str()), hl(i == *harness)));
                opts.push(Span::raw(" "));
            }
            let marker = |on: bool| if on { t.pointer() } else { " " };
            if *field == AddField::Count {
                cursor = Some((11 + count.width() as u16, 2));
            }
            (
                "Add accounts".into(),
                vec![
                    Line::from(
                        std::iter::once(Span::raw(format!(
                            "{} ",
                            marker(*field == AddField::Harness)
                        )))
                        .chain(opts)
                        .collect::<Vec<_>>(),
                    ),
                    Line::raw(""),
                    Line::from(vec![
                        Span::raw(format!("{} ", marker(*field == AddField::Count))),
                        Span::styled("Count    ", t.muted()),
                        Span::raw(count.clone()),
                        Span::styled("  (1-99, numbered from the lowest free number)", t.muted()),
                    ]),
                    Line::raw(""),
                    muted("←/→ harness   Tab next field   Enter add   Esc cancel"),
                ],
                68,
            )
        }
        Modal::Import(found) => {
            let mut v = vec![
                Line::raw(format!("Found {} login(s) not yet accounts:", found.len())),
                Line::raw(""),
            ];
            for c in found.iter().take(8) {
                v.push(Line::raw(format!("  {} ({})", c.name, c.harness)));
            }
            if found.len() > 8 {
                v.push(muted(&format!(
                    "  {} {} more",
                    t.ellipsis(),
                    found.len() - 8
                )));
            }
            v.push(Line::raw(""));
            v.push(muted("Enter import all   Esc cancel"));
            ("Import".into(), v, 56)
        }
    };
    let height = (lines.len() as u16 + 4).min(full.height);
    let rect = centered(width, height, full);
    f.render_widget(Clear, rect);
    let b =
        block(t, Span::styled(format!(" {title} "), t.accent())).padding(Padding::new(1, 1, 1, 1));
    let inner = b.inner(rect);
    f.render_widget(b, rect);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    if let Some((cx, cy)) = cursor {
        let x = (inner.x + cx).min(inner.right().saturating_sub(1));
        f.set_cursor_position((x, inner.y + cy));
    }
}
