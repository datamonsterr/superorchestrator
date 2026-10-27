//! Application state and key handling. Nothing here draws or does I/O: `on_key` and
//! `on_update` turn input into state changes and, for work, an [`Effect`] that the event
//! loop hands to a background thread.

use crate::cli::{LevelAdd, LevelEdit, LevelModelCmd, LevelsCmd};

use chrono::{DateTime, Duration, Utc};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use so_accounts::ImportCandidate;
use so_core::{Harness, RunState};

use super::data::{AccountRow, RunDetail, RunItem, Snapshot};
use super::theme::Theme;
use so_core::proto::RunDiff;

/// The harnesses an account can be added for, in the order the form lists them.
pub const HARNESSES: [Harness; 4] = [
    Harness::Codex,
    Harness::Opencode,
    Harness::Agy,
    Harness::Claude,
];

const PAGE: usize = 10;
const TOAST_OK: i64 = 4;
const TOAST_ERR: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Accounts,
    Runs,
    Setup,
    Levels,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Overview,
        Tab::Accounts,
        Tab::Runs,
        Tab::Setup,
        Tab::Levels,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Accounts => "Accounts",
            Tab::Runs => "Runs",
            Tab::Setup => "Setup",
            Tab::Levels => "Levels",
        }
    }
    fn step(self, by: isize) -> Tab {
        let n = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + by).rem_euclid(n) as usize]
    }
}

/// The setup steps (`cmd::setup`), in the order a first run needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    Init,
    Harnesses,
    Import,
    AddAccounts,
    SignIn,
    Install,
    Doctor,
}

impl SetupStep {
    pub const ALL: [SetupStep; 7] = [
        SetupStep::Init,
        SetupStep::Harnesses,
        SetupStep::Import,
        SetupStep::AddAccounts,
        SetupStep::SignIn,
        SetupStep::Install,
        SetupStep::Doctor,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub fn label(self) -> &'static str {
        match self {
            SetupStep::Init => "init",
            SetupStep::Harnesses => "harnesses",
            SetupStep::Import => "import",
            SetupStep::AddAccounts => "add accounts",
            SetupStep::SignIn => "sign in",
            SetupStep::Install => "install",
            SetupStep::Doctor => "doctor",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            SetupStep::Init => "create the config dir, config.toml, levels and catalog",
            SetupStep::Harnesses => "find the harness binaries on PATH",
            SetupStep::Import => "import logins found on disk as accounts",
            SetupStep::AddAccounts => "add one account for each installed harness without one",
            SetupStep::SignIn => "list accounts that are signed out and how to sign in",
            SetupStep::Install => "register the MCP server, skills and CLAUDE.md block",
            SetupStep::Doctor => "check binaries, config, database, socket and sign-ins",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Done,
    /// Finished, but something needs the user.
    Warn,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StepRow {
    pub state: StepState,
    pub lines: Vec<String>,
}

/// Work for a background thread. Reading is `Refresh` and `Detail`; the rest change state.
#[derive(Debug, Clone)]
pub enum Effect {
    Refresh,
    /// `diff` also asks the supervisor for the run's diff (only done for runs waiting for review).
    Detail {
        id: String,
        diff: bool,
    },
    AddAccounts {
        harness: Harness,
        count: u32,
    },
    Login(String),
    SetEnabled {
        name: String,
        enabled: bool,
    },
    /// `None` clears the cooldown.
    Cooldown {
        name: String,
        hours: Option<f64>,
    },
    Remove(String),
    ScanImport,
    ImportAll(Vec<ImportCandidate>),
    Cancel(String),
    Accept {
        run: String,
        merge: bool,
    },
    Reject {
        run: String,
        feedback: String,
    },
    Join(String),
    Setup(SetupStep),
    SetupAll,
    Levels(LevelsCmd),
}

impl Effect {
    /// Short phrase for the status line while the effect runs; `None` for reads.
    pub fn busy_label(&self) -> Option<&'static str> {
        Some(match self {
            Effect::Refresh | Effect::Detail { .. } => return None,
            Effect::AddAccounts { .. } => "adding accounts",
            Effect::Login(_) => "preparing sign-in",
            Effect::SetEnabled { .. } => "updating account",
            Effect::Cooldown { .. } => "updating cooldown",
            Effect::Remove(_) => "removing account",
            Effect::ScanImport => "scanning for logins",
            Effect::ImportAll(_) => "importing",
            Effect::Cancel(_) => "cancelling run",
            Effect::Accept { .. } => "accepting run",
            Effect::Reject { .. } => "rejecting run",
            Effect::Join(_) => "preparing join",
            Effect::Setup(_) | Effect::SetupAll => "running setup",
            Effect::Levels(_) => "updating levels",
        })
    }
}

/// A command to run with the terminal handed over to it (login, join).
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub title: String,
    /// Printed before the command runs.
    pub note: Option<String>,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
}

/// What a background thread reports back.
#[derive(Debug)]
pub enum Update {
    Snapshot(Box<Snapshot>),
    Detail(Result<Box<RunDetail>, String>),
    /// A read on the reader thread panicked or failed outright.
    ReadFailed(String),
    Done {
        ok: bool,
        text: String,
    },
    Hint {
        title: String,
        lines: Vec<String>,
    },
    Launch(Launch),
    Candidates(Vec<ImportCandidate>),
    Step {
        step: SetupStep,
        state: StepState,
        lines: Vec<String>,
    },
}

/// What the event loop does after an update.
#[derive(Debug, PartialEq)]
pub enum Next {
    Nothing,
    Refresh,
    Launch(Launch),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    RejectFeedback,
    CooldownHours,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddField {
    Harness,
    Count,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Help,
    Message {
        title: String,
        lines: Vec<String>,
    },
    /// `y` runs the effect; anything else cancels.
    Confirm {
        prompt: String,
        effect: Effect,
    },
    /// Removing needs the name typed.
    TypeName {
        name: String,
        typed: String,
    },
    Input {
        kind: InputKind,
        /// The run or account the input is for.
        target: String,
        title: String,
        typed: String,
    },
    AddAccount {
        harness: usize,
        count: String,
        field: AddField,
    },
    LevelForm {
        /// None creates a level; Some edits that level's range.
        target: Option<String>,
        fields: [String; 3],
        field: usize,
    },
    CatalogPicker {
        level: String,
        new_level: Option<LevelAdd>,
        refs: Vec<String>,
        selected: usize,
    },
    DeleteLevel {
        name: String,
        typed: String,
    },
    Import(Vec<ImportCandidate>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Ok,
    Err,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: DateTime<Utc>,
}

/// The Runs tab's open detail pane.
#[derive(Debug, Clone)]
pub struct DetailView {
    pub id: String,
    /// `None` while loading.
    pub data: Option<Result<RunDetail, String>>,
    pub scroll: u16,
    /// The diff, kept between refreshes; `Err` says why there is none.
    pub diff: Option<Result<RunDiff, String>>,
    /// What the run looked like when `diff` was fetched: (state, event count).
    pub diff_key: Option<(RunState, usize)>,
    /// Fetch the diff with the next detail read (R / F5, or the run changed).
    pub force_diff: bool,
}

pub struct App {
    pub theme: Theme,
    pub tab: Tab,
    pub now: DateTime<Utc>,
    pub tick: u64,
    pub snap: Option<Snapshot>,
    pub accounts_sel: usize,
    pub runs_sel: usize,
    pub setup_sel: usize,
    pub levels_sel: usize,
    pub models_sel: usize,
    pub level_open: Option<String>,
    pub levels_state: TableState,
    pub models_state: TableState,
    /// Scroll state of the two tables; the draw step keeps it on the selection.
    pub accounts_state: TableState,
    pub runs_state: TableState,
    pub detail: Option<DetailView>,
    pub setup: Vec<StepRow>,
    pub modal: Option<Modal>,
    pub toast: Option<Toast>,
    pub busy: Option<&'static str>,
    pub quit: bool,
}

impl App {
    pub fn new(theme: Theme, now: DateTime<Utc>) -> Self {
        Self {
            theme,
            tab: Tab::Overview,
            now,
            tick: 0,
            snap: None,
            accounts_sel: 0,
            runs_sel: 0,
            setup_sel: 0,
            levels_sel: 0,
            models_sel: 0,
            level_open: None,
            levels_state: TableState::default(),
            models_state: TableState::default(),
            accounts_state: TableState::default(),
            runs_state: TableState::default(),
            detail: None,
            setup: SetupStep::ALL
                .iter()
                .map(|_| StepRow {
                    state: StepState::Pending,
                    lines: Vec::new(),
                })
                .collect(),
            modal: None,
            toast: None,
            busy: None,
            quit: false,
        }
    }

    pub fn accounts(&self) -> &[AccountRow] {
        self.snap.as_ref().map_or(&[], |s| s.accounts.as_slice())
    }

    pub fn runs(&self) -> &[RunItem] {
        self.snap.as_ref().map_or(&[], |s| s.runs.as_slice())
    }

    pub fn selected_account(&self) -> Option<&AccountRow> {
        self.accounts().get(self.accounts_sel)
    }

    pub fn selected_run(&self) -> Option<&RunItem> {
        self.runs().get(self.runs_sel)
    }

    /// The run whose detail pane is open.
    #[cfg(test)]
    pub fn detail_id(&self) -> Option<&str> {
        self.detail.as_ref().map(|d| d.id.as_str())
    }

    fn say(&mut self, kind: ToastKind, text: impl Into<String>) {
        let secs = if kind == ToastKind::Ok {
            TOAST_OK
        } else {
            TOAST_ERR
        };
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            until: self.now + Duration::seconds(secs),
        });
    }

    /// Advances the clock; drops an expired toast. Returns whether the screen changed.
    pub fn on_tick(&mut self, now: DateTime<Utc>) -> bool {
        self.now = now;
        self.tick = self.tick.wrapping_add(1);
        let expired = self.toast.as_ref().is_some_and(|t| t.until <= now);
        if expired {
            self.toast = None;
        }
        expired || self.busy.is_some()
    }

    fn start(&mut self, effect: Effect) -> Option<Effect> {
        if let Some(label) = effect.busy_label() {
            self.busy = Some(label);
        }
        Some(effect)
    }

    // ------------------------------------------------------------------ keys

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return None;
        }
        if let Some(m) = self.modal.take() {
            return self.modal_key(m, key);
        }
        self.toast = None;
        if key.code == KeyCode::Char('?') {
            self.modal = Some(Modal::Help);
            return None;
        }
        let plain = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Tab => self.switch(self.tab.step(1)),
            KeyCode::BackTab => self.switch(self.tab.step(-1)),
            KeyCode::Char(c @ '1'..='5') if plain => {
                self.switch(Tab::ALL[usize::from(c as u8 - b'1')]);
            }
            KeyCode::F(5) => return self.refresh(),
            KeyCode::Char('R') if plain => return self.refresh(),
            KeyCode::Char('q') | KeyCode::Esc if plain => self.back_or_quit(),
            _ => {
                return match self.tab {
                    Tab::Overview => None,
                    Tab::Accounts => self.accounts_key(key),
                    Tab::Runs => self.runs_key(key),
                    Tab::Setup => self.setup_key(key),
                    Tab::Levels => self.levels_key(key),
                };
            }
        }
        None
    }

    /// R / F5: reload everything, including the open run's diff.
    fn refresh(&mut self) -> Option<Effect> {
        if let Some(d) = &mut self.detail {
            d.force_diff = true;
        }
        Some(Effect::Refresh)
    }

    /// The read that keeps the open run's detail fresh. It asks for the diff only the
    /// first time, after R / F5, or when the run changed since the diff was fetched.
    pub fn detail_request(&mut self) -> Option<Effect> {
        let d = self.detail.as_mut()?;
        let diff = std::mem::take(&mut d.force_diff);
        Some(Effect::Detail {
            id: d.id.clone(),
            diff,
        })
    }

    fn switch(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// `q` and `Esc`: close what is open, else leave.
    fn back_or_quit(&mut self) {
        if self.tab == Tab::Runs && self.detail.is_some() {
            self.detail = None;
        } else if self.tab == Tab::Levels && self.level_open.is_some() {
            self.level_open = None;
        } else {
            self.quit = true;
        }
    }

    /// Moves a selection for a list key; `None` when the key is not a movement.
    fn movement(code: KeyCode, sel: usize, len: usize) -> Option<usize> {
        let last = len.saturating_sub(1);
        Some(match code {
            KeyCode::Down | KeyCode::Char('j') => (sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => sel.saturating_sub(1),
            KeyCode::PageDown => (sel + PAGE).min(last),
            KeyCode::PageUp => sel.saturating_sub(PAGE),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => last,
            _ => return None,
        })
    }

    fn accounts_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if let Some(s) = Self::movement(key.code, self.accounts_sel, self.accounts().len()) {
            self.accounts_sel = s;
            return None;
        }
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        match c {
            'a' => {
                self.modal = Some(Modal::AddAccount {
                    harness: 0,
                    count: "1".into(),
                    field: AddField::Harness,
                });
                None
            }
            'i' => self.start(Effect::ScanImport),
            _ => {
                let acc = self.selected_account()?.clone();
                match c {
                    'l' => self.start(Effect::Login(acc.name)),
                    'e' => self.start(Effect::SetEnabled {
                        enabled: !acc.enabled,
                        name: acc.name,
                    }),
                    'f' if acc.cooling => self.start(Effect::Cooldown {
                        name: acc.name,
                        hours: None,
                    }),
                    'f' => {
                        self.modal = Some(Modal::Input {
                            kind: InputKind::CooldownHours,
                            title: format!("Flag {} in cooldown for how many hours?", acc.name),
                            target: acc.name,
                            typed: "5".into(),
                        });
                        None
                    }
                    'x' => {
                        self.modal = Some(Modal::TypeName {
                            name: acc.name,
                            typed: String::new(),
                        });
                        None
                    }
                    _ => None,
                }
            }
        }
    }

    fn runs_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if let Some(d) = &mut self.detail {
            // The detail pane scrolls instead of moving the selection.
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => d.scroll = d.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                KeyCode::PageDown => d.scroll = d.scroll.saturating_add(PAGE as u16),
                KeyCode::PageUp => d.scroll = d.scroll.saturating_sub(PAGE as u16),
                KeyCode::Home | KeyCode::Char('g') => d.scroll = 0,
                // The draw step clamps this to the end of the content.
                KeyCode::End | KeyCode::Char('G') => d.scroll = u16::MAX,
                _ => {}
            }
        } else if let Some(s) = Self::movement(key.code, self.runs_sel, self.runs().len()) {
            self.runs_sel = s;
            return None;
        } else if key.code == KeyCode::Enter {
            let id = self.selected_run()?.id.clone();
            self.detail = Some(DetailView {
                id: id.clone(),
                data: None,
                scroll: 0,
                diff: None,
                diff_key: None,
                force_diff: false,
            });
            return Some(Effect::Detail { id, diff: true });
        }
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        let run = self.run_in_focus()?;
        let short: String = run.0.chars().take(8).collect();
        match c {
            'c' if run.1.is_terminal() => {
                self.say(ToastKind::Err, format!("run {short} already {}", run.1));
                None
            }
            'c' => {
                self.modal = Some(Modal::Confirm {
                    prompt: format!("Cancel run {short} ({})?", run.1),
                    effect: Effect::Cancel(run.0),
                });
                None
            }
            'o' => self.start(Effect::Join(run.0)),
            'a' | 'A' | 'r' if run.1 != RunState::PendingReview => {
                self.say(
                    ToastKind::Err,
                    format!(
                        "run {short} is {}; only pending_review runs can be reviewed",
                        run.1
                    ),
                );
                None
            }
            'a' | 'A' => {
                // `a` merges when the run has a worktree to merge; `A` only accepts.
                let merge = c == 'a' && run.2;
                let prompt = if merge {
                    format!("Accept and merge run {short}? (y/n)")
                } else {
                    format!("Accept run {short} without merging? (y/n)")
                };
                self.modal = Some(Modal::Confirm {
                    prompt,
                    effect: Effect::Accept { run: run.0, merge },
                });
                None
            }
            'r' => {
                self.modal = Some(Modal::Input {
                    kind: InputKind::RejectFeedback,
                    title: format!("Feedback for run {short} (what to change)"),
                    target: run.0,
                    typed: String::new(),
                });
                None
            }
            _ => None,
        }
    }

    /// The open run, else the selected one, with its state.
    fn run_in_focus(&self) -> Option<(String, RunState, bool)> {
        let id = match &self.detail {
            Some(d) => d.id.as_str(),
            None => self.selected_run()?.id.as_str(),
        };
        let (state, worktree) = self
            .runs()
            .iter()
            .find(|r| r.id == id)
            .map(|r| (r.state, r.has_worktree))
            .or(match &self.detail {
                Some(DetailView {
                    data: Some(Ok(d)), ..
                }) => Some((d.state, d.worktree.is_some())),
                _ => None,
            })?;
        Some((id.to_string(), state, worktree))
    }

    pub fn levels(&self) -> &[super::data::LevelRow] {
        self.snap.as_ref().map_or(&[], |s| s.levels.as_slice())
    }

    pub fn focused_level(&self) -> Option<&super::data::LevelRow> {
        match &self.level_open {
            Some(name) => self.levels().iter().find(|l| &l.name == name),
            None => self.levels().get(self.levels_sel),
        }
    }

    fn catalog_picker(&mut self, level: String, new_level: Option<LevelAdd>) {
        let Some(s) = &self.snap else { return };
        if let Some(e) = &s.catalog_error {
            self.say(ToastKind::Err, e.clone());
            return;
        }
        let refs = s
            .catalog_refs
            .iter()
            .filter(|r| {
                new_level.is_some() || !self.focused_level().is_some_and(|l| l.models.contains(r))
            })
            .cloned()
            .collect::<Vec<_>>();
        if refs.is_empty() {
            self.say(ToastKind::Err, "no catalog model refs available to add");
            return;
        }
        self.modal = Some(Modal::CatalogPicker {
            level,
            new_level,
            refs,
            selected: 0,
        });
    }

    fn levels_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if let Some(e) = self.snap.as_ref().and_then(|s| s.levels_error.clone()) {
            self.say(ToastKind::Err, e);
        }
        if self.busy.is_some() {
            return None;
        }
        let open = self.level_open.is_some();
        let len = if open {
            self.focused_level().map_or(0, |l| l.models.len())
        } else {
            self.levels().len()
        };
        let sel = if open {
            self.models_sel
        } else {
            self.levels_sel
        };
        if let Some(next) = Self::movement(key.code, sel, len) {
            if open {
                self.models_sel = next;
            } else {
                self.levels_sel = next;
            }
            return None;
        }
        match key.code {
            KeyCode::Char('n') => {
                self.modal = Some(Modal::LevelForm {
                    target: None,
                    fields: [String::new(), "0".into(), "100".into()],
                    field: 0,
                });
                return None;
            }
            KeyCode::Char('u') => {
                self.modal = Some(Modal::Confirm {
                    prompt: "Reset levels to defaults? (y/n)".into(),
                    effect: Effect::Levels(LevelsCmd::Reset),
                });
                return None;
            }
            _ => {}
        }
        let l = self.focused_level()?.clone();
        let cmd = match key.code {
            KeyCode::Enter if !open => {
                self.level_open = Some(l.name);
                self.models_sel = 0;
                return None;
            }
            KeyCode::Char('a') => {
                self.catalog_picker(l.name, None);
                return None;
            }
            KeyCode::Char('r') => {
                self.modal = Some(Modal::LevelForm {
                    target: Some(l.name.clone()),
                    fields: [l.name, l.min.to_string(), l.max.to_string()],
                    field: 1,
                });
                return None;
            }
            KeyCode::Char('D') => {
                self.modal = Some(Modal::DeleteLevel {
                    name: l.name,
                    typed: String::new(),
                });
                return None;
            }
            KeyCode::Char('x') if open => LevelsCmd::Model(LevelModelCmd::Remove {
                level: l.name,
                model: l.models.get(sel)?.clone(),
            }),
            KeyCode::Char(c @ ('J' | 'K')) if open => {
                let to = if c == 'J' {
                    (sel + 1).min(len.saturating_sub(1))
                } else {
                    sel.saturating_sub(1)
                };
                if to == sel {
                    return None;
                }
                LevelsCmd::Model(LevelModelCmd::Move {
                    level: l.name,
                    model: l.models.get(sel)?.clone(),
                    position: to + 1,
                })
            }
            _ => return None,
        };
        self.start(Effect::Levels(cmd))
    }

    fn setup_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if let Some(s) = Self::movement(key.code, self.setup_sel, SetupStep::ALL.len()) {
            self.setup_sel = s;
            return None;
        }
        match key.code {
            KeyCode::Enter => self.run_step(SetupStep::ALL[self.setup_sel]),
            KeyCode::Char('i') => {
                self.setup_sel = SetupStep::Install.index();
                self.run_step(SetupStep::Install)
            }
            KeyCode::Char('d') => {
                self.setup_sel = SetupStep::Doctor.index();
                self.run_step(SetupStep::Doctor)
            }
            KeyCode::Char('a') => {
                for row in &mut self.setup {
                    row.state = StepState::Pending;
                    row.lines.clear();
                }
                self.start(Effect::SetupAll)
            }
            _ => None,
        }
    }

    fn run_step(&mut self, step: SetupStep) -> Option<Effect> {
        if self.busy.is_some() {
            self.say(ToastKind::Err, "wait for the current step to finish");
            return None;
        }
        self.start(Effect::Setup(step))
    }

    // ---------------------------------------------------------------- modals

    fn modal_key(&mut self, mut modal: Modal, key: KeyEvent) -> Option<Effect> {
        let code = key.code;
        match &mut modal {
            Modal::LevelForm {
                target,
                fields,
                field,
            } => {
                match code {
                    KeyCode::Esc => return None,
                    KeyCode::Tab | KeyCode::Down => {
                        *field = if *field == 2 {
                            usize::from(target.is_some())
                        } else {
                            *field + 1
                        }
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        *field = if *field == usize::from(target.is_some()) {
                            2
                        } else {
                            *field - 1
                        }
                    }
                    KeyCode::Enter => {
                        let range = fields[1]
                            .parse::<u8>()
                            .ok()
                            .zip(fields[2].parse::<u8>().ok());
                        if let Some((min, max)) =
                            range.filter(|(min, max)| min <= max && *max <= 100)
                        {
                            if let Some(name) = target {
                                return self.start(Effect::Levels(LevelsCmd::Edit(LevelEdit {
                                    name: name.clone(),
                                    new_name: None,
                                    min: Some(min),
                                    max: Some(max),
                                    tier: None,
                                    no_tier: false,
                                })));
                            }
                            let name = fields[0].trim().to_string();
                            if !name.is_empty() {
                                self.toast = None;
                                self.catalog_picker(
                                    name.clone(),
                                    Some(LevelAdd {
                                        name,
                                        min,
                                        max,
                                        tier: None,
                                        models: Vec::new(),
                                    }),
                                );
                                return None;
                            }
                            self.say(ToastKind::Err, "level name must not be empty");
                        } else {
                            self.say(ToastKind::Err, "range needs min <= max <= 100");
                        }
                    }
                    _ => edit(&mut fields[*field], code, if *field == 0 { 64 } else { 3 }),
                }
                self.modal = Some(modal);
                None
            }
            Modal::CatalogPicker {
                level,
                new_level,
                refs,
                selected,
            } => {
                if code == KeyCode::Esc {
                    return None;
                }
                if let Some(next) = Self::movement(code, *selected, refs.len()) {
                    *selected = next;
                }
                if code == KeyCode::Enter {
                    let model = refs.get(*selected)?.clone();
                    let cmd = if let Some(mut a) = new_level.take() {
                        a.models.push(model);
                        LevelsCmd::Add(a)
                    } else {
                        LevelsCmd::Model(LevelModelCmd::Add {
                            level: level.clone(),
                            model,
                            position: None,
                        })
                    };
                    return self.start(Effect::Levels(cmd));
                }
                self.modal = Some(modal);
                None
            }
            Modal::DeleteLevel { name, typed } => match code {
                KeyCode::Esc => None,
                KeyCode::Enter if typed == name => {
                    let name = name.clone();
                    self.start(Effect::Levels(LevelsCmd::Remove { name }))
                }
                KeyCode::Enter => {
                    self.say(ToastKind::Err, "name did not match; nothing removed");
                    None
                }
                _ => {
                    edit(typed, code, name.chars().count().max(64));
                    self.modal = Some(modal);
                    None
                }
            },
            Modal::Help | Modal::Message { .. } => {
                if !matches!(
                    code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?')
                ) {
                    self.modal = Some(modal);
                }
                None
            }
            Modal::Confirm { effect, .. } => {
                if code == KeyCode::Char('y') {
                    let effect = effect.clone();
                    return self.start(effect);
                }
                if !matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n')) {
                    self.modal = Some(modal);
                }
                None
            }
            Modal::TypeName { name, typed } => match code {
                KeyCode::Esc => None,
                KeyCode::Enter if typed.trim() == name => {
                    let name = name.clone();
                    self.start(Effect::Remove(name))
                }
                KeyCode::Enter => {
                    self.say(ToastKind::Err, "name did not match; nothing removed");
                    None
                }
                _ => {
                    edit(typed, code, 64);
                    self.modal = Some(modal);
                    None
                }
            },
            Modal::Input {
                kind,
                target,
                typed,
                ..
            } => match code {
                KeyCode::Esc => None,
                KeyCode::Enter => {
                    let (kind, target, text) = (*kind, target.clone(), typed.trim().to_string());
                    self.submit_input(modal, kind, target, text)
                }
                _ => {
                    edit(typed, code, 400);
                    self.modal = Some(modal);
                    None
                }
            },
            Modal::AddAccount {
                harness,
                count,
                field,
            } => match code {
                KeyCode::Esc => None,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    *field = match field {
                        AddField::Harness => AddField::Count,
                        AddField::Count => AddField::Harness,
                    };
                    self.modal = Some(modal);
                    None
                }
                KeyCode::Left | KeyCode::Right if *field == AddField::Harness => {
                    let n = HARNESSES.len();
                    *harness = if code == KeyCode::Left {
                        (*harness + n - 1) % n
                    } else {
                        (*harness + 1) % n
                    };
                    self.modal = Some(modal);
                    None
                }
                KeyCode::Enter => match count.parse::<u32>() {
                    Ok(n) if (1..=99).contains(&n) => {
                        let harness = HARNESSES[*harness];
                        self.start(Effect::AddAccounts { harness, count: n })
                    }
                    _ => {
                        self.say(ToastKind::Err, "enter a count from 1 to 99");
                        self.modal = Some(modal);
                        None
                    }
                },
                KeyCode::Char(c) if *field == AddField::Count && !c.is_ascii_digit() => {
                    self.modal = Some(modal);
                    None
                }
                _ => {
                    if *field == AddField::Count {
                        edit(count, code, 2);
                    }
                    self.modal = Some(modal);
                    None
                }
            },
            Modal::Import(found) => match code {
                KeyCode::Esc => None,
                KeyCode::Enter => {
                    let found = found.clone();
                    self.start(Effect::ImportAll(found))
                }
                _ => {
                    self.modal = Some(modal);
                    None
                }
            },
        }
    }

    fn submit_input(
        &mut self,
        modal: Modal,
        kind: InputKind,
        target: String,
        text: String,
    ) -> Option<Effect> {
        match kind {
            InputKind::RejectFeedback if text.is_empty() => {
                self.say(ToastKind::Err, "feedback cannot be empty");
                self.modal = Some(modal);
                None
            }
            InputKind::RejectFeedback => self.start(Effect::Reject {
                run: target,
                feedback: text,
            }),
            InputKind::CooldownHours => match text.parse::<f64>() {
                Ok(h) if h.is_finite() && h > 0.0 && h <= 24.0 * 365.0 => {
                    self.start(Effect::Cooldown {
                        name: target,
                        hours: Some(h),
                    })
                }
                _ => {
                    self.say(ToastKind::Err, "hours must be a positive number");
                    self.modal = Some(modal);
                    None
                }
            },
        }
    }

    // --------------------------------------------------------------- updates

    pub fn on_update(&mut self, update: Update) -> Next {
        match update {
            Update::Snapshot(s) => {
                self.apply_snapshot(*s);
                Next::Nothing
            }
            Update::Detail(r) => {
                if let Some(d) = &mut self.detail {
                    match r {
                        Ok(v) if v.id == d.id => {
                            let mut v = *v;
                            let waiting =
                                v.state == RunState::PendingReview && v.worktree.is_some();
                            let key = (v.state, v.events_total);
                            match v.diff.take() {
                                Some(diff) => {
                                    d.diff = Some(diff);
                                    d.diff_key = Some(key);
                                    d.force_diff = false;
                                }
                                None if !waiting => {
                                    d.diff = None;
                                    d.diff_key = None;
                                    d.force_diff = false;
                                }
                                None if d.diff_key != Some(key) => d.force_diff = true,
                                None => {}
                            }
                            d.data = Some(Ok(v));
                        }
                        Ok(_) => {}
                        Err(e) => d.data = Some(Err(e)),
                    }
                }
                Next::Nothing
            }
            Update::ReadFailed(e) => {
                self.say(ToastKind::Err, e);
                Next::Nothing
            }
            Update::Done { ok, text } => {
                self.busy = None;
                if !text.is_empty() {
                    self.say(if ok { ToastKind::Ok } else { ToastKind::Err }, text);
                }
                Next::Refresh
            }
            Update::Hint { title, lines } => {
                self.busy = None;
                self.modal = Some(Modal::Message { title, lines });
                Next::Nothing
            }
            Update::Launch(l) => {
                self.busy = None;
                Next::Launch(l)
            }
            Update::Candidates(found) => {
                self.busy = None;
                if found.is_empty() {
                    self.say(ToastKind::Ok, "no new logins found on disk");
                } else {
                    self.modal = Some(Modal::Import(found));
                }
                Next::Nothing
            }
            Update::Step { step, state, lines } => {
                if let Some(row) = self.setup.get_mut(step.index()) {
                    row.state = state;
                    row.lines = lines;
                }
                Next::Nothing
            }
        }
    }

    /// Keeps each selection on the same account or run when the lists change.
    fn apply_snapshot(&mut self, snap: Snapshot) {
        let acc = self.selected_account().map(|a| a.name.clone());
        let run = self.selected_run().map(|r| r.id.clone());
        self.accounts_sel = acc
            .and_then(|n| snap.accounts.iter().position(|a| a.name == n))
            .unwrap_or(self.accounts_sel)
            .min(snap.accounts.len().saturating_sub(1));
        self.runs_sel = run
            .and_then(|id| snap.runs.iter().position(|r| r.id == id))
            .unwrap_or(self.runs_sel)
            .min(snap.runs.len().saturating_sub(1));
        let level = self.levels().get(self.levels_sel).map(|l| l.name.clone());
        self.levels_sel = level
            .and_then(|n| snap.levels.iter().position(|l| l.name == n))
            .unwrap_or(self.levels_sel)
            .min(snap.levels.len().saturating_sub(1));
        if let Some(name) = &self.level_open {
            if let Some(l) = snap.levels.iter().find(|l| &l.name == name) {
                // Follow the selected ref through reorders and refreshes.
                let model = self
                    .focused_level()
                    .and_then(|l| l.models.get(self.models_sel));
                self.models_sel = model
                    .and_then(|r| l.models.iter().position(|m| m == r))
                    .unwrap_or(self.models_sel)
                    .min(l.models.len().saturating_sub(1));
            } else {
                self.level_open = None;
                self.models_sel = 0;
            }
        }
        self.snap = Some(snap);
    }
}

/// Text-field editing: characters, Backspace; everything else is ignored.
fn edit(text: &mut String, code: KeyCode, max: usize) {
    match code {
        KeyCode::Char(c) if !c.is_control() && text.chars().count() < max => text.push(c),
        KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}
