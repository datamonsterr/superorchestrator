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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
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

/// Why the UI should stop (a signal), and whether a child owns the terminal right now.
#[derive(Default)]
struct Stop {
    code: AtomicI32,
    child: AtomicBool,
}

impl Stop {
    /// Asks the loop to stop. True for the first request.
    fn request(&self, code: i32) -> bool {
        self.code
            .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn requested(&self) -> Option<i32> {
        match self.code.load(Ordering::SeqCst) {
            0 => None,
            c => Some(c),
        }
    }

    /// SIGINT: the child it was meant for gets it while it runs; otherwise it is a stop.
    fn interrupt(&self) -> bool {
        !self.child.load(Ordering::SeqCst) && self.request(130)
    }
}

type PanicHook = Arc<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>>;

/// Replaces ratatui's panic hook: a panic on the UI thread restores the terminal first, a
/// panic on a worker thread does not (the UI is still running) and prints nothing, because
/// the worker reports it through its channel instead.
fn set_panic_hook(ui_thread: std::thread::ThreadId, orig: PanicHook) {
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            ratatui::restore();
            orig(info);
        }
    }));
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
    // Not joined at the end: it may be waiting (up to a few seconds) on the supervisor.
    {
        let (c, h, u) = (ctx.clone(), host.clone(), utx.clone());
        std::thread::spawn(move || effects::read_loop(c, h, rrx, u));
    }
    let actor = {
        let (c, h) = (ctx.clone(), host);
        std::thread::spawn(move || effects::action_loop(c, h, arx, utx))
    };
    let stop = Arc::new(Stop::default());
    watch_signals(stop.clone());

    let ui_thread = std::thread::current().id();
    let orig: PanicHook = Arc::new(std::panic::take_hook());
    {
        let o = orig.clone();
        std::panic::set_hook(Box::new(move |i| o(i)));
    }
    let result = {
        let _restore = Restore;
        // `try_init` wraps the hook above with its own restore; replace that.
        let mut terminal = ratatui::try_init()?;
        set_panic_hook(ui_thread, orig);
        event_loop(&mut terminal, &rtx, &atx, &urx, &stop)
    };
    // Let a step that is writing config finish rather than cut it off.
    drop((rtx, atx));
    if !actor.is_finished() {
        eprintln!("finishing the running step...");
    }
    let _ = actor.join();
    if let Some(code) = stop.requested() {
        std::process::exit(code);
    }
    result.map_err(Into::into)
}

/// Where the loop sends work: reads and actions run on separate threads, so a slow install
/// never delays the refresh.
struct Dispatch<'a> {
    reads: &'a Sender<Effect>,
    actions: &'a Sender<Effect>,
    refreshing: bool,
    detail_in_flight: bool,
    last_refresh: Instant,
}

impl Dispatch<'_> {
    /// False when the thread that should do the work is gone.
    fn send(&mut self, effect: Effect) -> bool {
        match effect {
            Effect::Refresh => {
                if self.refreshing {
                    return true;
                }
                self.refreshing = true;
                self.last_refresh = Instant::now();
                self.reads.send(Effect::Refresh).is_ok()
            }
            Effect::Detail { .. } => {
                if self.detail_in_flight {
                    return true;
                }
                self.detail_in_flight = true;
                self.reads.send(effect).is_ok()
            }
            other => self.actions.send(other).is_ok(),
        }
    }

    /// A read finished (or failed): the next one may go out.
    fn on_update(&mut self, u: &Update) {
        match u {
            Update::Snapshot(_) => self.refreshing = false,
            Update::Detail(_) => self.detail_in_flight = false,
            Update::ReadFailed(_) => {
                self.refreshing = false;
                self.detail_in_flight = false;
            }
            _ => {}
        }
    }
}

const WORKER_GONE: &str = "a background worker stopped; restart the terminal UI";

fn event_loop(
    terminal: &mut DefaultTerminal,
    reads: &Sender<Effect>,
    actions: &Sender<Effect>,
    updates: &mpsc::Receiver<Update>,
    stop: &Stop,
) -> io::Result<()> {
    let mut app = App::new(Theme::from_env(), Utc::now());
    let mut d = Dispatch {
        reads,
        actions,
        refreshing: false,
        detail_in_flight: false,
        last_refresh: Instant::now(),
    };
    let mut worker_gone = !d.send(Effect::Refresh);
    let mut dirty = true;
    let mut last_draw = Instant::now();
    while !app.quit && stop.requested().is_none() {
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
        let mut sent = true;
        if event::poll(wait)? {
            loop {
                match event::read()? {
                    Event::Key(k) => {
                        if let Some(effect) = app.on_key(k) {
                            let refresh = matches!(effect, Effect::Refresh);
                            sent &= d.send(effect);
                            if refresh && let Some(e) = app.detail_request() {
                                sent &= d.send(e);
                            }
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
            d.on_update(&u);
            match app.on_update(u) {
                Next::Nothing => {}
                Next::Refresh => sent &= d.send(Effect::Refresh),
                Next::Launch(l) => {
                    run_launch(terminal, &l, stop)?;
                    sent &= d.send(Effect::Refresh);
                }
            }
            dirty = true;
        }
        if app.on_tick(Utc::now()) {
            dirty = true;
        }
        if d.last_refresh.elapsed() >= REFRESH && !d.refreshing {
            sent &= d.send(Effect::Refresh);
            if let Some(e) = app.detail_request() {
                sent &= d.send(e);
            }
        }
        if !sent && !worker_gone {
            // Say it once; the screen stays usable for what still works.
            worker_gone = true;
            app.on_update(Update::Done {
                ok: false,
                text: WORKER_GONE.into(),
            });
            dirty = true;
        }
    }
    Ok(())
}

/// Hands the terminal to a child (sign-in, join) and takes it back afterwards. A stop
/// request kills the child and returns.
fn run_launch(terminal: &mut DefaultTerminal, l: &Launch, stop: &Stop) -> io::Result<()> {
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
    so_core::child_env::isolate(&mut cmd, &l.pass_env);
    cmd.args(&l.args).envs(l.env.iter().map(|(k, v)| (k, v)));
    if let Some(c) = &l.cwd {
        cmd.current_dir(c);
    }
    stop.child.store(true, Ordering::SeqCst);
    let status = cmd.spawn().and_then(|mut child| {
        loop {
            if let Some(s) = child.try_wait()? {
                break Ok(s);
            }
            if stop.requested().is_some() {
                let _ = child.kill();
                let s = child.wait()?;
                break Ok(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    stop.child.store(false, Ordering::SeqCst);
    match status {
        Ok(s) if s.success() => writeln!(out, "\nfinished.")?,
        Ok(s) => writeln!(out, "\nexited with {s}.")?,
        Err(e) => writeln!(out, "\ncould not run {}: {e}", l.program)?,
    }
    if stop.requested().is_some() {
        return Ok(());
    }
    write!(out, "Press Enter to return to superorchestrator.")?;
    out.flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()
}

/// SIGTERM and SIGHUP stop the loop, which restores the terminal and lets the running step
/// finish; if that takes more than two seconds the process exits anyway. SIGINT is ignored
/// while a child owns the terminal (it is meant for the child) and otherwise stops the UI.
/// In raw mode Ctrl-C is an ordinary key.
fn watch_signals(stop: Arc<Stop>) {
    use tokio::signal::unix::{SignalKind, signal};
    std::thread::spawn(move || {
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
                let (first, code) = tokio::select! {
                    _ = term.recv() => (stop.request(143), 143),
                    _ = hup.recv() => (stop.request(129), 129),
                    _ = int.recv() => (stop.interrupt(), 130),
                };
                if first {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(2));
                        ratatui::restore();
                        std::process::exit(code);
                    });
                }
            }
        });
    });
}
