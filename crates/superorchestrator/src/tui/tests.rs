//! Key handling, state transitions and `TestBackend` render snapshots.

use chrono::{DateTime, TimeZone, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier};
use so_accounts::{ImportCandidate, SignIn};
use so_core::{Harness, RunState};

use super::app::*;
use super::data::*;
use super::theme::Theme;
use super::ui;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap()
}

fn account(name: &str, harness: Harness, signed_in: SignIn, enabled: bool) -> AccountRow {
    AccountRow {
        name: name.into(),
        harness,
        signed_in,
        enabled,
        cap: 2,
        active: 0,
        headroom: String::new(),
        cooling: false,
    }
}

fn run(id: &str, state: RunState, who: &str, secs_ago: i64, title: &str) -> RunItem {
    RunItem {
        id: id.into(),
        state,
        harness: Harness::Codex,
        account: Some(who.into()),
        created_at: now() - chrono::Duration::seconds(secs_ago),
        title: title.into(),
    }
}

fn snapshot() -> Snapshot {
    let mut codex = account("codex-1", Harness::Codex, SignIn::SignedIn, true);
    codex.headroom = "codex: ok 82%".into();
    codex.active = 1;
    let mut cool = account("codex-2", Harness::Codex, SignIn::SignedIn, true);
    cool.headroom = "codex: cooldown until 2026-10-23T15:00:00+00:00".into();
    cool.cooling = true;
    Snapshot {
        at: now() - chrono::Duration::seconds(3),
        config_dir: "~/.config/superorchestrator".into(),
        supervisor: Some(SupInfo {
            pid: 4242,
            version: "0.1.0".into(),
            uptime_s: 3720,
        }),
        accounts: vec![
            codex,
            cool,
            account("opencode-1", Harness::Opencode, SignIn::SignedOut, true),
            account("claude-1", Harness::Claude, SignIn::Unknown, false),
        ],
        accounts_error: None,
        runs: vec![
            run(
                "0190aaaa-1111",
                RunState::Running,
                "codex-1",
                45,
                "Refactor the parser",
            ),
            run(
                "0190bbbb-2222",
                RunState::PendingReview,
                "codex-2",
                400,
                "Add retry to the HTTP client",
            ),
            run(
                "0190cccc-3333",
                RunState::Succeeded,
                "opencode-1",
                7300,
                "Write the changelog entry for 0.1.0",
            ),
            run(
                "0190dddd-4444",
                RunState::Failed,
                "codex-1",
                90_000,
                "Migrate the schema",
            ),
        ],
        active_runs: 2,
        pending_review: 1,
        checks: vec![
            Check {
                label: "config",
                level: Level::Ok,
                detail: "~/.config/superorchestrator".into(),
                fix: None,
            },
            Check {
                label: "harnesses",
                level: Level::Ok,
                detail: "codex, opencode (2 of 4)".into(),
                fix: None,
            },
            Check {
                label: "accounts",
                level: Level::Ok,
                detail: "4 configured".into(),
                fix: None,
            },
            Check {
                label: "signed in",
                level: Level::Missing,
                detail: "2 of 3 enabled".into(),
                fix: Some("Accounts tab (2): select one, press l".into()),
            },
            Check {
                label: "Claude Code",
                level: Level::Missing,
                detail: "not installed".into(),
                fix: Some("Setup tab (4): press i, or `superorchestrator install`".into()),
            },
            Check {
                label: "router key",
                level: Level::Warn,
                detail: "no OpenRouter key; routing uses the heuristic".into(),
                fix: Some("export OPENROUTER_API_KEY (optional)".into()),
            },
        ],
    }
}

fn detail() -> RunDetail {
    RunDetail {
        id: "0190bbbb-2222".into(),
        state: RunState::PendingReview,
        harness: Harness::Codex,
        account: Some("codex-2".into()),
        model: Some("gpt-5".into()),
        worktree: Some("/work/repo/.wt/0190bbbb".into()),
        branch: Some("so/0190bbbb".into()),
        error: None,
        brief: "Add retry with backoff to the HTTP client.\nKeep the public API unchanged.".into(),
        route: vec![
            "jev / codex-fast / codex-2 / gpt-5".into(),
            "small, well-scoped edit".into(),
        ],
        events: vec![
            "12:00:01 run_started".into(),
            "12:03:10 progress tests green".into(),
        ],
        result: Some("{\"summary\":\"done\"}".into()),
    }
}

fn app() -> App {
    let mut a = App::new(Theme::plain(), now());
    a.snap = Some(snapshot());
    a
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

/// Feeds keys; returns the last effect produced.
fn press(a: &mut App, keys: &[KeyEvent]) -> Option<Effect> {
    let mut last = None;
    for k in keys {
        last = a.on_key(*k);
    }
    last
}

fn type_text(a: &mut App, text: &str) {
    for c in text.chars() {
        a.on_key(ch(c));
    }
}

// ----------------------------------------------------------------- navigation

#[test]
fn tab_keys_cycle_and_digits_jump() {
    let mut a = app();
    assert_eq!(a.tab, Tab::Overview);
    press(&mut a, &[key(KeyCode::Tab)]);
    assert_eq!(a.tab, Tab::Accounts);
    press(&mut a, &[key(KeyCode::BackTab), key(KeyCode::BackTab)]);
    assert_eq!(a.tab, Tab::Setup, "shift-tab wraps backwards");
    press(&mut a, &[key(KeyCode::Tab)]);
    assert_eq!(a.tab, Tab::Overview, "tab wraps forwards");
    for (c, tab) in [
        ('2', Tab::Accounts),
        ('3', Tab::Runs),
        ('4', Tab::Setup),
        ('1', Tab::Overview),
    ] {
        press(&mut a, &[ch(c)]);
        assert_eq!(a.tab, tab);
    }
}

#[test]
fn q_and_esc_quit_at_the_top_level() {
    for k in [ch('q'), key(KeyCode::Esc)] {
        let mut a = app();
        a.on_key(k);
        assert!(a.quit);
    }
    let mut a = app();
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(a.quit, "ctrl-c quits");
}

#[test]
fn esc_closes_the_innermost_layer_first() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Enter)]);
    assert!(a.detail.is_some());
    a.on_key(ch('?'));
    assert!(matches!(a.modal, Some(Modal::Help)));
    a.on_key(key(KeyCode::Esc));
    assert!(a.modal.is_none() && a.detail.is_some() && !a.quit);
    a.on_key(key(KeyCode::Esc));
    assert!(a.detail.is_none() && !a.quit, "esc leaves the detail pane");
    a.on_key(key(KeyCode::Esc));
    assert!(a.quit);
}

#[test]
fn ctrl_c_quits_even_with_a_modal_open() {
    let mut a = app();
    a.on_key(ch('?'));
    a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(a.quit);
}

#[test]
fn key_releases_are_ignored() {
    let mut a = app();
    let mut k = ch('q');
    k.kind = KeyEventKind::Release;
    assert!(a.on_key(k).is_none());
    assert!(!a.quit);
}

#[test]
fn refresh_keys_ask_for_a_refresh() {
    let mut a = app();
    assert!(matches!(a.on_key(ch('R')), Some(Effect::Refresh)));
    assert!(matches!(
        a.on_key(key(KeyCode::F(5))),
        Some(Effect::Refresh)
    ));
}

#[test]
fn help_opens_everywhere_and_closes_with_esc_or_question_mark() {
    for tab in ['1', '2', '3', '4'] {
        let mut a = app();
        press(&mut a, &[ch(tab), ch('?')]);
        assert!(matches!(a.modal, Some(Modal::Help)));
        a.on_key(ch('?'));
        assert!(a.modal.is_none());
    }
}

#[test]
fn a_modal_swallows_keys_meant_for_the_tab() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('?')]);
    a.on_key(ch('3'));
    assert_eq!(
        a.tab,
        Tab::Accounts,
        "tab keys do nothing while help is open"
    );
    assert!(matches!(a.modal, Some(Modal::Help)));
}

// ------------------------------------------------------------------- accounts

#[test]
fn accounts_selection_stays_inside_the_list() {
    let mut a = app();
    a.on_key(ch('2'));
    press(&mut a, &[key(KeyCode::Up)]);
    assert_eq!(a.accounts_sel, 0);
    for _ in 0..10 {
        a.on_key(key(KeyCode::Down));
    }
    assert_eq!(a.accounts_sel, 3);
    a.on_key(key(KeyCode::Home));
    assert_eq!(a.accounts_sel, 0);
    a.on_key(key(KeyCode::End));
    assert_eq!(a.accounts_sel, 3);
    a.on_key(key(KeyCode::PageUp));
    assert_eq!(a.accounts_sel, 0);
}

#[test]
fn add_form_picks_a_harness_and_a_count() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('a')]);
    assert!(matches!(
        a.modal,
        Some(Modal::AddAccount { harness: 0, .. })
    ));
    // codex -> opencode, then edit the count.
    press(&mut a, &[key(KeyCode::Right), key(KeyCode::Tab)]);
    press(&mut a, &[key(KeyCode::Backspace)]);
    type_text(&mut a, "x3");
    let Some(Effect::AddAccounts { harness, count }) = a.on_key(key(KeyCode::Enter)) else {
        panic!("expected AddAccounts");
    };
    assert_eq!((harness, count), (Harness::Opencode, 3));
    assert!(a.modal.is_none());
    assert_eq!(a.busy, Some("adding accounts"));
}

#[test]
fn add_form_rejects_a_bad_count_and_stays_open() {
    let mut a = app();
    press(
        &mut a,
        &[
            ch('2'),
            ch('a'),
            key(KeyCode::Tab),
            key(KeyCode::Backspace),
            ch('0'),
        ],
    );
    assert!(a.on_key(key(KeyCode::Enter)).is_none());
    assert!(a.modal.is_some(), "the form stays open");
    assert_eq!(a.toast.as_ref().map(|t| t.kind), Some(ToastKind::Err));
    // Empty is rejected too; Esc cancels without work.
    a.on_key(key(KeyCode::Backspace));
    assert!(a.on_key(key(KeyCode::Enter)).is_none());
    a.on_key(key(KeyCode::Esc));
    assert!(a.modal.is_none() && a.busy.is_none());
}

#[test]
fn add_form_wraps_the_harness_choice() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('a'), key(KeyCode::Left)]);
    assert!(matches!(
        a.modal,
        Some(Modal::AddAccount { harness: 3, .. })
    ));
    a.on_key(key(KeyCode::Right));
    assert!(matches!(
        a.modal,
        Some(Modal::AddAccount { harness: 0, .. })
    ));
}

#[test]
fn enable_toggles_the_selected_account() {
    let mut a = app();
    a.on_key(ch('2'));
    let Some(Effect::SetEnabled { name, enabled }) = a.on_key(ch('e')) else {
        panic!()
    };
    assert_eq!((name.as_str(), enabled), ("codex-1", false));
    a.accounts_sel = 3;
    let Some(Effect::SetEnabled { name, enabled }) = a.on_key(ch('e')) else {
        panic!()
    };
    assert_eq!((name.as_str(), enabled), ("claude-1", true));
}

#[test]
fn remove_needs_the_exact_name() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('x')]);
    assert!(matches!(a.modal, Some(Modal::TypeName { .. })));
    type_text(&mut a, "codex");
    assert!(a.on_key(key(KeyCode::Enter)).is_none());
    assert!(a.modal.is_none(), "a wrong name closes the prompt");
    assert_eq!(a.toast.as_ref().map(|t| t.kind), Some(ToastKind::Err));
    assert!(a.busy.is_none());

    press(&mut a, &[ch('x')]);
    type_text(&mut a, "codex-1");
    let Some(Effect::Remove(name)) = a.on_key(key(KeyCode::Enter)) else {
        panic!()
    };
    assert_eq!(name, "codex-1");
}

#[test]
fn remove_can_be_cancelled() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('x')]);
    type_text(&mut a, "codex-1");
    assert!(a.on_key(key(KeyCode::Esc)).is_none());
    assert!(a.modal.is_none() && a.busy.is_none() && !a.quit);
}

#[test]
fn flag_asks_for_hours_then_sets_a_cooldown() {
    let mut a = app();
    press(&mut a, &[ch('2'), ch('f')]);
    assert!(matches!(
        a.modal,
        Some(Modal::Input {
            kind: InputKind::CooldownHours,
            ..
        })
    ));
    // The field starts at 5; make it 2.5.
    press(&mut a, &[key(KeyCode::Backspace)]);
    type_text(&mut a, "2.5");
    let Some(Effect::Cooldown { name, hours }) = a.on_key(key(KeyCode::Enter)) else {
        panic!()
    };
    assert_eq!((name.as_str(), hours), ("codex-1", Some(2.5)));
}

#[test]
fn flag_rejects_nonsense_hours() {
    for bad in ["", "abc", "-1", "0", "NaN"] {
        let mut a = app();
        press(&mut a, &[ch('2'), ch('f'), key(KeyCode::Backspace)]);
        type_text(&mut a, bad);
        assert!(a.on_key(key(KeyCode::Enter)).is_none(), "{bad:?}");
        assert!(a.modal.is_some(), "{bad:?} keeps the prompt");
    }
}

#[test]
fn flag_on_a_cooling_account_clears_it() {
    let mut a = app();
    press(&mut a, &[ch('2'), key(KeyCode::Down)]);
    let Some(Effect::Cooldown { name, hours }) = a.on_key(ch('f')) else {
        panic!()
    };
    assert_eq!((name.as_str(), hours), ("codex-2", None));
}

#[test]
fn login_and_import_start_work() {
    let mut a = app();
    a.on_key(ch('2'));
    assert!(matches!(a.on_key(ch('l')), Some(Effect::Login(n)) if n == "codex-1"));
    assert!(matches!(a.on_key(ch('i')), Some(Effect::ScanImport)));
}

#[test]
fn account_keys_with_no_accounts_do_nothing_harmful() {
    let mut a = app();
    a.snap.as_mut().unwrap().accounts.clear();
    a.on_key(ch('2'));
    for c in ['l', 'e', 'f', 'x'] {
        assert!(a.on_key(ch(c)).is_none());
    }
    assert!(a.modal.is_none());
    // Adding still works.
    a.on_key(ch('a'));
    assert!(matches!(a.modal, Some(Modal::AddAccount { .. })));
}

// ----------------------------------------------------------------------- runs

#[test]
fn enter_opens_the_detail_and_asks_for_it() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Down)]);
    let Some(Effect::Detail(id)) = a.on_key(key(KeyCode::Enter)) else {
        panic!()
    };
    assert_eq!(id, "0190bbbb-2222");
    assert_eq!(a.detail_id(), Some("0190bbbb-2222"));
    // Arrow keys scroll the pane instead of moving the list.
    a.on_key(key(KeyCode::Down));
    assert_eq!(a.detail.as_ref().unwrap().scroll, 1);
    assert_eq!(a.runs_sel, 1);
    a.on_update(Update::Detail(Ok(Box::new(detail()))));
    assert!(matches!(a.detail.as_ref().unwrap().data, Some(Ok(_))));
}

#[test]
fn a_stale_detail_for_another_run_is_dropped() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Enter)]);
    a.on_update(Update::Detail(Ok(Box::new(detail()))));
    assert!(a.detail.as_ref().unwrap().data.is_none());
}

#[test]
fn cancel_confirms_before_acting() {
    let mut a = app();
    press(&mut a, &[ch('3'), ch('c')]);
    assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
    assert!(a.on_key(ch('n')).is_none());
    assert!(a.modal.is_none() && a.busy.is_none());

    a.on_key(ch('c'));
    // Enter is "no": only an explicit y acts.
    assert!(a.on_key(key(KeyCode::Enter)).is_none());
    assert!(a.busy.is_none());
    a.on_key(ch('c'));
    let Some(Effect::Cancel(id)) = a.on_key(ch('y')) else {
        panic!()
    };
    assert_eq!(id, "0190aaaa-1111");
}

#[test]
fn finished_runs_cannot_be_cancelled() {
    let mut a = app();
    press(
        &mut a,
        &[ch('3'), key(KeyCode::Down), key(KeyCode::Down), ch('c')],
    );
    assert!(a.modal.is_none());
    assert_eq!(a.toast.as_ref().map(|t| t.kind), Some(ToastKind::Err));
}

#[test]
fn only_runs_waiting_for_review_can_be_accepted_or_rejected() {
    let mut a = app();
    press(&mut a, &[ch('3'), ch('a')]);
    assert!(a.modal.is_none(), "the first run is running");
    assert!(a.toast.as_ref().unwrap().text.contains("pending_review"));
    a.on_key(ch('r'));
    assert!(a.modal.is_none());

    a.on_key(key(KeyCode::Down));
    a.on_key(ch('a'));
    assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
    let Some(Effect::Accept(id)) = a.on_key(ch('y')) else {
        panic!()
    };
    assert_eq!(id, "0190bbbb-2222");
}

#[test]
fn reject_asks_for_feedback_and_refuses_an_empty_one() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Down), ch('r')]);
    assert!(matches!(
        a.modal,
        Some(Modal::Input {
            kind: InputKind::RejectFeedback,
            ..
        })
    ));
    assert!(a.on_key(key(KeyCode::Enter)).is_none());
    assert!(a.modal.is_some(), "empty feedback keeps the prompt open");
    type_text(&mut a, "  use jittered backoff ");
    let Some(Effect::Reject { run, feedback }) = a.on_key(key(KeyCode::Enter)) else {
        panic!()
    };
    assert_eq!(run, "0190bbbb-2222");
    assert_eq!(feedback, "use jittered backoff");
}

#[test]
fn join_and_actions_inside_the_detail_pane_use_the_open_run() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Down), key(KeyCode::Enter)]);
    assert!(matches!(a.on_key(ch('j')), Some(Effect::Join(id)) if id == "0190bbbb-2222"));
    a.on_key(ch('a'));
    assert!(matches!(
        a.modal,
        Some(Modal::Confirm {
            effect: Effect::Accept(_),
            ..
        })
    ));
}

#[test]
fn selection_follows_the_run_when_new_runs_arrive() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Down)]);
    assert_eq!(a.selected_run().unwrap().id, "0190bbbb-2222");
    let mut s = snapshot();
    s.runs.insert(
        0,
        run("0190eeee-5555", RunState::Queued, "codex-1", 1, "new"),
    );
    a.on_update(Update::Snapshot(Box::new(s)));
    assert_eq!(a.selected_run().unwrap().id, "0190bbbb-2222");
    assert_eq!(a.runs_sel, 2);
}

#[test]
fn selection_is_clamped_when_the_lists_shrink() {
    let mut a = app();
    a.accounts_sel = 3;
    a.runs_sel = 3;
    let mut s = snapshot();
    s.accounts.truncate(1);
    s.runs.clear();
    a.on_update(Update::Snapshot(Box::new(s)));
    assert_eq!((a.accounts_sel, a.runs_sel), (0, 0));
    assert!(a.selected_run().is_none());
    a.on_key(ch('3'));
    assert!(a.on_key(key(KeyCode::Enter)).is_none(), "no run, no detail");
}

// ---------------------------------------------------------------------- setup

#[test]
fn setup_runs_the_selected_step_and_one_at_a_time() {
    let mut a = app();
    a.on_key(ch('4'));
    a.on_key(key(KeyCode::Down));
    assert!(matches!(
        a.on_key(key(KeyCode::Enter)),
        Some(Effect::Setup(SetupStep::Harnesses))
    ));
    assert!(a.busy.is_some());
    assert!(
        a.on_key(key(KeyCode::Enter)).is_none(),
        "busy: no second step"
    );
    assert_eq!(a.toast.as_ref().map(|t| t.kind), Some(ToastKind::Err));
}

#[test]
fn setup_shortcuts_select_install_and_doctor() {
    let mut a = app();
    a.on_key(ch('4'));
    assert!(matches!(
        a.on_key(ch('i')),
        Some(Effect::Setup(SetupStep::Install))
    ));
    assert_eq!(a.setup_sel, SetupStep::Install.index());
    a.busy = None;
    assert!(matches!(
        a.on_key(ch('d')),
        Some(Effect::Setup(SetupStep::Doctor))
    ));
}

#[test]
fn run_all_resets_the_rows_and_steps_report_back() {
    let mut a = app();
    a.on_key(ch('4'));
    a.on_update(Update::Step {
        step: SetupStep::Init,
        state: StepState::Failed,
        lines: vec!["boom".into()],
    });
    assert_eq!(a.setup[0].state, StepState::Failed);
    assert!(matches!(a.on_key(ch('a')), Some(Effect::SetupAll)));
    assert!(
        a.setup
            .iter()
            .all(|r| r.state == StepState::Pending && r.lines.is_empty())
    );
    a.on_update(Update::Step {
        step: SetupStep::Doctor,
        state: StepState::Warn,
        lines: vec!["1 warning".into()],
    });
    assert_eq!(a.setup[SetupStep::Doctor.index()].state, StepState::Warn);
}

// -------------------------------------------------------------------- updates

#[test]
fn done_clears_busy_shows_a_toast_and_asks_for_a_refresh() {
    let mut a = app();
    a.busy = Some("removing account");
    let next = a.on_update(Update::Done {
        ok: false,
        text: "boom".into(),
    });
    assert_eq!(next, Next::Refresh);
    assert!(a.busy.is_none());
    let t = a.toast.clone().unwrap();
    assert_eq!((t.kind, t.text.as_str()), (ToastKind::Err, "boom"));
}

#[test]
fn toasts_expire() {
    let mut a = app();
    a.on_update(Update::Done {
        ok: true,
        text: "ok".into(),
    });
    assert!(!a.on_tick(now() + chrono::Duration::seconds(1)));
    assert!(a.toast.is_some());
    assert!(a.on_tick(now() + chrono::Duration::seconds(5)));
    assert!(a.toast.is_none());
}

#[test]
fn errors_stay_longer_than_successes() {
    let mut a = app();
    a.on_update(Update::Done {
        ok: true,
        text: "ok".into(),
    });
    let ok_until = a.toast.as_ref().unwrap().until;
    a.on_update(Update::Done {
        ok: false,
        text: "bad".into(),
    });
    assert!(a.toast.as_ref().unwrap().until > ok_until);
}

#[test]
fn a_keypress_dismisses_the_toast() {
    let mut a = app();
    a.on_update(Update::Done {
        ok: false,
        text: "bad".into(),
    });
    a.on_key(ch('2'));
    assert!(a.toast.is_none());
}

#[test]
fn hints_and_launches_come_through() {
    let mut a = app();
    a.busy = Some("preparing sign-in");
    a.on_update(Update::Hint {
        title: "Sign in".into(),
        lines: vec!["run x".into()],
    });
    assert!(matches!(a.modal, Some(Modal::Message { .. })));
    assert!(a.busy.is_none());
    a.on_key(key(KeyCode::Enter));
    assert!(a.modal.is_none());

    let l = Launch {
        title: "t".into(),
        note: None,
        program: "codex".into(),
        args: vec!["login".into()],
        env: vec![],
        cwd: None,
    };
    assert_eq!(a.on_update(Update::Launch(l.clone())), Next::Launch(l));
}

#[test]
fn import_candidates_open_a_confirmation_and_then_import() {
    let mut a = app();
    assert_eq!(a.on_update(Update::Candidates(vec![])), Next::Nothing);
    assert!(a.modal.is_none() && a.toast.is_some());
    let found = vec![ImportCandidate {
        name: "codex-9".into(),
        harness: Harness::Codex,
        home: "/h/.codex-9".into(),
    }];
    a.on_update(Update::Candidates(found.clone()));
    assert!(matches!(a.modal, Some(Modal::Import(_))));
    let Some(Effect::ImportAll(got)) = a.on_key(key(KeyCode::Enter)) else {
        panic!()
    };
    assert_eq!(got, found);
}

// --------------------------------------------------------------------- render

fn text(buf: &Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < area.width {
                let sym = buf[(x, y)].symbol();
                row.push_str(sym);
                // A wide character owns the next cell too.
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            row.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(a: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    t.backend().buffer().clone()
}

fn shot(a: &mut App, w: u16, h: u16) -> String {
    text(&render(a, w, h))
}

macro_rules! sizes {
    ($name:ident, $setup:expr) => {
        #[test]
        fn $name() {
            for (w, h) in [(80, 24), (120, 40)] {
                let mut a = app();
                let setup: fn(&mut App) = $setup;
                setup(&mut a);
                insta::assert_snapshot!(
                    format!("{}_{w}x{h}", stringify!($name)),
                    shot(&mut a, w, h)
                );
            }
        }
    };
}

sizes!(overview, |_| {});
sizes!(accounts, |a| {
    a.on_key(ch('2'));
    a.on_key(key(KeyCode::Down));
});
sizes!(runs, |a| {
    a.on_key(ch('3'));
    a.on_key(key(KeyCode::Down));
});
sizes!(run_detail, |a| {
    press(a, &[ch('3'), key(KeyCode::Down), key(KeyCode::Enter)]);
    a.on_update(Update::Detail(Ok(Box::new(detail()))));
});
sizes!(setup, |a| {
    a.on_key(ch('4'));
    a.on_update(Update::Step {
        step: SetupStep::Init,
        state: StepState::Done,
        lines: vec!["~/.config/superorchestrator already set up".into()],
    });
    a.on_update(Update::Step {
        step: SetupStep::Harnesses,
        state: StepState::Done,
        lines: vec![
            "codex     /usr/bin/codex (2 account(s))".into(),
            "claude    not installed".into(),
        ],
    });
    a.on_update(Update::Step {
        step: SetupStep::SignIn,
        state: StepState::Warn,
        lines: vec!["1 account(s) signed out.".into()],
    });
    a.on_update(Update::Step {
        step: SetupStep::Install,
        state: StepState::Failed,
        lines: vec!["1 step(s) failed".into()],
    });
    a.setup_sel = 1;
});

#[test]
fn modals_render_at_80x24() {
    type Case = (&'static str, fn(&mut App));
    let cases: Vec<Case> = vec![
        ("help_overview", |a| {
            a.on_key(ch('?'));
        }),
        ("help_accounts", |a| {
            press(a, &[ch('2'), ch('?')]);
        }),
        ("add_account", |a| {
            press(a, &[ch('2'), ch('a'), key(KeyCode::Right)]);
        }),
        ("confirm_cancel", |a| {
            press(a, &[ch('3'), ch('c')]);
        }),
        ("remove_account", |a| {
            press(a, &[ch('2'), ch('x')]);
            type_text(a, "codex");
        }),
        ("reject_feedback", |a| {
            press(a, &[ch('3'), key(KeyCode::Down), ch('r')]);
            type_text(a, "use jitter");
        }),
        ("login_hint", |a| {
            a.on_update(Update::Hint {
                title: "Sign in to claude-1".into(),
                lines: vec![
                    "to sign in, run: claude-1".into(),
                    "note: run `/login` inside the session".into(),
                ],
            });
        }),
        ("import", |a| {
            a.on_update(Update::Candidates(vec![ImportCandidate {
                name: "codex-3".into(),
                harness: Harness::Codex,
                home: "/h/.codex-3".into(),
            }]));
        }),
    ];
    for (name, setup) in cases {
        let mut a = app();
        setup(&mut a);
        insta::assert_snapshot!(format!("modal_{name}"), shot(&mut a, 80, 24));
    }
}

#[test]
fn busy_and_toast_show_in_the_status_line() {
    let mut a = app();
    a.busy = Some("adding accounts");
    let s = shot(&mut a, 80, 24);
    assert!(s.contains("adding accounts"), "{s}");
    a.busy = None;
    a.on_update(Update::Done {
        ok: false,
        text: "supervisor not running — start with any delegation or `superorchestrator daemon`"
            .into(),
    });
    let s = shot(&mut a, 80, 24);
    assert!(s.contains("supervisor not running"), "{s}");
    assert!(s.contains('✗'));
}

#[test]
fn empty_states_say_what_to_do() {
    let mut a = app();
    {
        let s = a.snap.as_mut().unwrap();
        s.accounts.clear();
        s.runs.clear();
        s.supervisor = None;
        s.active_runs = 0;
        s.pending_review = 0;
    }
    a.on_key(ch('2'));
    insta::assert_snapshot!("empty_accounts_80x24", shot(&mut a, 80, 24));
    a.on_key(ch('3'));
    insta::assert_snapshot!("empty_runs_80x24", shot(&mut a, 80, 24));
}

#[test]
fn loading_before_the_first_snapshot() {
    let mut a = App::new(Theme::plain(), now());
    for k in ['1', '2', '3'] {
        a.on_key(ch(k));
        let s = shot(&mut a, 80, 24);
        assert!(s.contains("loading"), "tab {k}: {s}");
    }
}

#[test]
fn accounts_read_error_is_shown_not_dropped() {
    let mut a = app();
    a.snap.as_mut().unwrap().accounts_error = Some("bad toml in codex-1.toml".into());
    a.on_key(ch('2'));
    assert!(shot(&mut a, 80, 24).contains("bad toml"));
}

#[test]
fn too_small_terminals_get_a_message_not_a_broken_layout() {
    let mut a = app();
    let s = shot(&mut a, 40, 10);
    assert!(s.contains("too small") && s.contains("60x16"), "{s}");
    // And recovers on resize.
    assert!(shot(&mut a, 80, 24).contains("Overview"));
}

#[test]
fn every_tab_survives_awkward_sizes() {
    for (w, h) in [(60, 16), (61, 17), (80, 16), (200, 60), (60, 60)] {
        for k in ['1', '2', '3', '4'] {
            let mut a = app();
            a.on_key(ch(k));
            let _ = shot(&mut a, w, h);
            press(&mut a, &[ch('3'), key(KeyCode::Enter)]);
            a.on_update(Update::Detail(Ok(Box::new(detail()))));
            let _ = shot(&mut a, w, h);
            a.on_key(ch('?'));
            let _ = shot(&mut a, w, h);
        }
    }
}

#[test]
fn long_text_is_truncated_with_an_ellipsis_inside_its_column() {
    let mut a = app();
    {
        let s = a.snap.as_mut().unwrap();
        s.accounts[0].name = "a-very-long-account-name-indeed".into();
        s.runs[0].title = "x".repeat(200);
        s.runs[1].title = "日本語のタイトルがとても長い場合でも切り詰められる".repeat(3);
    }
    a.on_key(ch('2'));
    let s = shot(&mut a, 80, 24);
    assert!(s.contains("a-very-long-accou…"), "{s}");
    a.on_key(ch('3'));
    let s = shot(&mut a, 80, 24);
    assert!(
        s.lines()
            .all(|l| unicode_width::UnicodeWidthStr::width(l) <= 80),
        "{s}"
    );
    assert!(s.contains('…'));
}

#[test]
fn control_characters_in_titles_cannot_reach_the_screen() {
    assert_eq!(clean("hi\x1b[2Jthere"), "hi [2Jthere");
}

#[test]
fn selected_rows_are_reversed_and_status_has_a_symbol_without_colour() {
    let mut a = App::new(
        Theme::detect(|k| match k {
            "NO_COLOR" => Some("1".into()),
            "LANG" => Some("en_US.UTF-8".into()),
            _ => None,
        }),
        now(),
    );
    a.snap = Some(snapshot());
    a.on_key(ch('2'));
    let buf = render(&mut a, 80, 24);
    let s = text(&buf);
    // Row 0 of the table is selected: find its cells and check the modifier.
    let y = (0..buf.area.height)
        .find(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, *y)].symbol())
                .collect::<String>()
                .contains("codex-1")
        })
        .unwrap();
    let x = (0..buf.area.width)
        .find(|x| buf[(*x, y)].symbol() == "c")
        .unwrap();
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
    assert!(s.contains("✗ no") && s.contains("✓ yes"), "{s}");
    // NO_COLOR: no cell carries a colour.
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            assert_eq!((c.fg, c.bg), (Color::Reset, Color::Reset), "({x},{y})");
        }
    }
}

#[test]
fn colour_is_used_when_allowed() {
    let theme = Theme {
        color: true,
        unicode: true,
    };
    let mut a = App::new(theme, now());
    a.snap = Some(snapshot());
    let buf = render(&mut a, 80, 24);
    let colored = (0..24)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .any(|(x, y)| buf[(x, y)].fg != Color::Reset);
    assert!(colored);
}

#[test]
fn ascii_terminals_get_no_unicode_symbols() {
    let theme = Theme {
        color: false,
        unicode: false,
    };
    let mut a = App::new(theme, now());
    a.snap = Some(snapshot());
    for k in ['1', '2', '3', '4'] {
        a.on_key(ch(k));
        let s = shot(&mut a, 80, 24);
        // Hint keys use arrows; everything else must be ASCII.
        let body: String = s.lines().take(21).collect::<Vec<_>>().join("\n");
        assert!(body.is_ascii(), "tab {k}:\n{body}");
    }
}

#[test]
fn age_formats() {
    let n = now();
    let a = |s| ui::age(n, n - chrono::Duration::seconds(s));
    assert_eq!(
        (a(5), a(125), a(7300), a(200_000), a(-5)),
        (
            "5s".into(),
            "2m".into(),
            "2h".into(),
            "2d".into(),
            "0s".into()
        )
    );
}

#[test]
fn truncate_counts_display_width() {
    assert_eq!(ui::truncate("hello", 10, "…"), "hello");
    assert_eq!(ui::truncate("hello world", 8, "…"), "hello w…");
    assert_eq!(ui::truncate("日本語日本語", 7, "…"), "日本語…");
    assert_eq!(ui::truncate("abc", 0, "…"), "");
}

#[test]
fn detail_scroll_is_clamped_to_the_content_and_hints_show_more() {
    let mut a = app();
    press(&mut a, &[ch('3'), key(KeyCode::Down), key(KeyCode::Enter)]);
    a.on_update(Update::Detail(Ok(Box::new(detail()))));
    assert!(
        shot(&mut a, 80, 24).contains("more"),
        "content below the fold is signalled"
    );
    for _ in 0..200 {
        a.on_key(key(KeyCode::Down));
    }
    let s = shot(&mut a, 80, 24);
    let scroll = a.detail.as_ref().unwrap().scroll;
    assert!(scroll < 40, "clamped, was {scroll}");
    assert!(s.contains("Result"), "{s}");
    a.on_key(key(KeyCode::Up));
    assert_eq!(a.detail.as_ref().unwrap().scroll, scroll - 1);
}

#[test]
fn modals_replace_the_footer_hints() {
    let mut a = app();
    a.on_key(ch('?'));
    let s = shot(&mut a, 80, 24);
    assert!(s.lines().last().unwrap().contains("Esc close"), "{s}");
    assert!(!s.lines().last().unwrap().contains("refresh"));
}
