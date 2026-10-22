//! The full-screen terminal UI (`superorchestrator` on a terminal, or `tui`).
//!
//! `app` holds the state and key handling, `ui` draws it, `effects` does the work on
//! background threads, and this module owns the terminal and the event loop.

mod app;
mod data;
mod effects;
#[cfg(test)]
mod tests;
mod theme;
mod ui;

use std::io::{self, BufRead, Write};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

use app::{App, Effect, Launch, Next, Update};
use theme::Theme;

use crate::ctx::{Ctx, interactive};
use crate::exit::{CliError, USAGE};

/// How often the screen reloads from the store.
const REFRESH: Duration = Duration::from_secs(2);

/// Restores the terminal on every way out of [`run`], including `?` and unwinding.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn run(ctx: &Ctx) -> Result<()> {
    if !interactive() {
        return Err(CliError::new(
            USAGE,
            "the terminal UI needs a terminal; run a subcommand instead (see --help)",
        )
        .into());
    }
    let host = ctx.host()?;
    let (utx, urx) = mpsc::channel::<Update>();
    let (rtx, rrx) = mpsc::channel::<Effect>();
    let (atx, arx) = mpsc::channel::<Effect>();
    let reader = {
        let (c, h, u) = (ctx.clone(), host.clone(), utx.clone());
        std::thread::spawn(move || effects::read_loop(c, h, rrx, u))
    };
    let actor = {
        let (c, h) = (ctx.clone(), host);
        std::thread::spawn(move || effects::action_loop(c, h, arx, utx))
    };
    watch_signals();

    let result = {
        let _restore = Restore;
        let mut terminal = ratatui::try_init()?;
        event_loop(&mut terminal, &rtx, &atx, &urx)
    };
    // Let a step that is writing config finish rather than cut it off.
    drop((rtx, atx));
    if !actor.is_finished() {
        eprintln!("finishing the running step...");
    }
    let _ = actor.join();
    let _ = reader.join();
    result.map_err(Into::into)
}

/// Where the loop sends work: reads and actions run on separate threads, so a slow install
/// never delays the refresh.
struct Dispatch<'a> {
    reads: &'a Sender<Effect>,
    actions: &'a Sender<Effect>,
    refreshing: bool,
    last_refresh: Instant,
}

impl Dispatch<'_> {
    fn send(&mut self, effect: Effect) {
        match effect {
            Effect::Refresh => {
                if !self.refreshing {
                    self.refreshing = true;
                    self.last_refresh = Instant::now();
                    let _ = self.reads.send(Effect::Refresh);
                }
            }
            Effect::Detail(_) => {
                let _ = self.reads.send(effect);
            }
            other => {
                let _ = self.actions.send(other);
            }
        }
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    reads: &Sender<Effect>,
    actions: &Sender<Effect>,
    updates: &mpsc::Receiver<Update>,
) -> io::Result<()> {
    let mut app = App::new(Theme::from_env(), Utc::now());
    let mut d = Dispatch {
        reads,
        actions,
        refreshing: false,
        last_refresh: Instant::now(),
    };
    d.send(Effect::Refresh);
    let mut dirty = true;
    let mut last_draw = Instant::now();
    while !app.quit {
        if dirty || last_draw.elapsed() >= Duration::from_secs(1) {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            dirty = false;
            last_draw = Instant::now();
        }
        let wait = if app.busy.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(250)
        };
        if event::poll(wait)? {
            loop {
                match event::read()? {
                    Event::Key(k) => {
                        if let Some(effect) = app.on_key(k) {
                            d.send(effect);
                        }
                    }
                    Event::Resize(..) => {}
                    _ => {}
                }
                dirty = true;
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        while let Ok(u) = updates.try_recv() {
            if matches!(u, Update::Snapshot(_)) {
                d.refreshing = false;
            }
            match app.on_update(u) {
                Next::Nothing => {}
                Next::Refresh => d.send(Effect::Refresh),
                Next::Launch(l) => {
                    run_launch(terminal, &l)?;
                    d.send(Effect::Refresh);
                }
            }
            dirty = true;
        }
        if app.on_tick(Utc::now()) {
            dirty = true;
        }
        if d.last_refresh.elapsed() >= REFRESH && !d.refreshing {
            d.send(Effect::Refresh);
            if let Some(id) = app.detail_id() {
                d.send(Effect::Detail(id.to_string()));
            }
        }
    }
    Ok(())
}

/// Hands the terminal to a child (sign-in, join) and takes it back afterwards.
fn run_launch(terminal: &mut DefaultTerminal, l: &Launch) -> io::Result<()> {
    terminal.show_cursor()?;
    ratatui::try_restore()?;
    let mut out = io::stdout();
    writeln!(out, "{}", l.title)?;
    if let Some(n) = &l.note {
        writeln!(out, "note: {n}")?;
    }
    writeln!(out)?;
    out.flush()?;
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args).envs(l.env.iter().map(|(k, v)| (k, v)));
    if let Some(c) = &l.cwd {
        cmd.current_dir(c);
    }
    match cmd.status() {
        Ok(s) if s.success() => writeln!(out, "\nfinished.")?,
        Ok(s) => writeln!(out, "\nexited with {s}.")?,
        Err(e) => writeln!(out, "\ncould not run {}: {e}", l.program)?,
    }
    write!(out, "Press Enter to return to superorchestrator.")?;
    out.flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()
}

/// Restores the terminal when the process is terminated or its terminal hangs up, and keeps
/// a Ctrl-C meant for a child (sign-in) from killing the UI while the child runs. In raw
/// mode Ctrl-C is an ordinary key.
fn watch_signals() {
    use tokio::signal::unix::{SignalKind, signal};
    std::thread::spawn(|| {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async {
            let (Ok(mut term), Ok(mut hup), Ok(mut int)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::hangup()),
                signal(SignalKind::interrupt()),
            ) else {
                return;
            };
            loop {
                tokio::select! {
                    _ = term.recv() => { ratatui::restore(); std::process::exit(143) }
                    _ = hup.recv() => { ratatui::restore(); std::process::exit(129) }
                    _ = int.recv() => {}
                }
            }
        });
    });
}
