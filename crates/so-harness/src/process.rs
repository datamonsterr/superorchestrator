//! Process pipeline shared by drivers that own a child process: spawn in its
//! own process group, read stdout and stderr with capped lines, record every
//! line in the transcript before emitting it, serialise stdin writes through
//! an actor, reap the leader and kill the group.

use std::fs::DirBuilder;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::driver::{Channel, DriverError, HarnessEvent, ShutdownOutcome};
use crate::transcript::{Dir, SharedTranscript, TranscriptWriter};

/// Longest line kept from a harness; the rest of the line is dropped.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Appended to a line that was cut at [`MAX_LINE_BYTES`].
pub const TRUNCATED_MARK: &str = "...[truncated]";

/// Creates `dir` (and parents) with mode 0700 and forces 0700 on `dir` itself.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Writes `bytes` to `path` with mode 0600. The mode is set before any data
/// is written, so an existing wider file is tightened first.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.set_len(0)?;
    f.write_all(bytes)
}

/// The event for a line cut at [`MAX_LINE_BYTES`] when the parser has nothing
/// better to say.
pub fn default_truncated_event() -> HarnessEvent {
    HarnessEvent::Error {
        kind: so_core::ErrorKind::Other,
        message: "line exceeded 4 MiB".into(),
    }
}

/// Turns stdout lines into normalised events.
pub trait LineParser: Send + 'static {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent>;

    /// A line on `channel` exceeded [`MAX_LINE_BYTES`] and was dropped
    /// unparsed. Parsers that track turn state should close open turns here.
    fn on_truncated(&mut self, _channel: Channel) -> Vec<HarnessEvent> {
        vec![default_truncated_event()]
    }

    /// The stream ended (EOF or read error).
    fn on_eof(&mut self) {}
}

/// How many stderr lines and bytes of a failed start are kept in the error text.
const STARTUP_TAIL_LINES: usize = 10;
const STARTUP_TAIL_BYTES: usize = 500;

/// Turns a failed start-up (the harness never became ready) into a
/// [`DriverError::Exited`] when the process itself said why: the tail of its stderr,
/// classified by the driver. The caller has already shut the process down, so the
/// stream ends with `Exited`; a process that wrote nothing keeps `cause` as it was.
pub(crate) async fn startup_failure(
    cause: DriverError,
    events: &mut mpsc::UnboundedReceiver<HarnessEvent>,
    classify: impl Fn(&str) -> so_core::ErrorKind,
) -> DriverError {
    if !matches!(cause, DriverError::Other(_)) {
        return cause;
    }
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let drain = async {
        while let Some(ev) = events.recv().await {
            match ev {
                HarnessEvent::Raw {
                    channel: Channel::Stderr,
                    line,
                    ..
                } if !line.trim().is_empty() => {
                    if tail.len() == STARTUP_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line.trim().to_string());
                }
                HarnessEvent::Exited { .. } => break,
                _ => {}
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), drain).await;
    if tail.is_empty() {
        return cause;
    }
    let mut text = tail.into_iter().collect::<Vec<_>>().join("; ");
    if text.len() > STARTUP_TAIL_BYTES {
        let mut at = text.len() - STARTUP_TAIL_BYTES;
        while !text.is_char_boundary(at) {
            at += 1;
        }
        text = format!("...{}", &text[at..]);
    }
    DriverError::Exited {
        kind: classify(&text),
        text: format!("{cause}: {text}"),
    }
}

/// `None` while the leader runs, `Some(code)` once reaped.
pub type ExitWatch = watch::Receiver<Option<Option<i32>>>;

pub(crate) async fn exited(exit: &ExitWatch, within: Duration) -> Option<Option<i32>> {
    let mut rx = exit.clone();
    match tokio::time::timeout(within, rx.wait_for(|v| v.is_some())).await {
        Ok(Ok(v)) => *v,
        // Sender dropped without a status: the waiter is gone, treat as exited.
        Ok(Err(_)) => Some(None),
        Err(_) => None,
    }
}

#[derive(Debug)]
struct GroupState {
    pgid: Option<rustix::process::Pid>,
    /// Set once the leader was reaped and leftovers were killed; the pgid may
    /// be reused from then on, so nothing signals it any more.
    reaped: AtomicBool,
}

impl GroupState {
    fn signal(&self, sig: rustix::process::Signal) {
        if self.reaped.load(Ordering::SeqCst) {
            return;
        }
        if let Some(pgid) = self.pgid {
            let _ = rustix::process::kill_process_group(pgid, sig);
        }
    }

    fn alive(&self) -> bool {
        !self.reaped.load(Ordering::SeqCst)
            && self
                .pgid
                .is_some_and(|p| rustix::process::test_kill_process_group(p).is_ok())
    }

    /// Called by the waiter right after the leader was reaped: while a member
    /// survives the pgid cannot be reused, so SIGKILL the leftovers now, then
    /// disarm for good.
    fn kill_leftovers_and_disarm(&self) {
        if self.alive() {
            self.signal(rustix::process::Signal::KILL);
        }
        self.reaped.store(true, Ordering::SeqCst);
    }
}

/// Owns the process group of a harness started with `process_group(0)`, so
/// grandchildren (tool subprocesses, wrappers) die with it. When the leader
/// exits (by itself or killed), the waiter kills any leftover members at once
/// and disarms the guard, so dropping it later never signals a possibly
/// reused pgid. Dropping the guard while the leader is still alive SIGKILLs
/// the group.
#[derive(Debug)]
pub struct ProcessGuard {
    state: Arc<GroupState>,
    kill_leader: Mutex<Option<oneshot::Sender<()>>>,
}

impl ProcessGuard {
    fn new(pid: Option<u32>, kill_leader: oneshot::Sender<()>) -> (Self, Arc<GroupState>) {
        let pgid = pid
            .and_then(|p| i32::try_from(p).ok())
            .and_then(rustix::process::Pid::from_raw);
        let state = Arc::new(GroupState {
            pgid,
            reaped: AtomicBool::new(false),
        });
        (
            Self {
                state: state.clone(),
                kill_leader: Mutex::new(Some(kill_leader)),
            },
            state,
        )
    }

    /// Whether any process of the (still armed) group exists.
    pub fn group_alive(&self) -> bool {
        self.state.alive()
    }

    /// SIGTERM the group once (no escalation).
    pub fn term(&self) {
        self.state.signal(rustix::process::Signal::TERM);
    }

    fn kill_leader(&self) {
        if let Ok(mut k) = self.kill_leader.lock()
            && let Some(tx) = k.take()
        {
            let _ = tx.send(());
        }
    }

    /// SIGTERM the group, SIGKILL after `grace` if members remain, and as a
    /// backstop make the waiter kill the leader directly. A no-op once the
    /// leader was reaped (leftovers are already dead).
    pub async fn terminate(&self, grace: Duration) {
        if self.state.alive() {
            self.state.signal(rustix::process::Signal::TERM);
            let deadline = tokio::time::Instant::now() + grace;
            while self.state.alive() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if self.state.alive() {
                self.state.signal(rustix::process::Signal::KILL);
            }
        }
        self.kill_leader();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.state.alive() {
            self.state.signal(rustix::process::Signal::KILL);
        }
        self.kill_leader();
    }
}

enum StdinCmd {
    Line(String, oneshot::Sender<Result<(), DriverError>>),
    Close,
}

/// Serialised access to the child's stdin, owned by an actor task. Cheap to
/// clone; the callers never share a lock, and the actor ends (closing stdin)
/// when the last handle drops.
#[derive(Debug, Clone)]
pub struct StdinHandle {
    tx: mpsc::UnboundedSender<StdinCmd>,
}

impl StdinHandle {
    /// Writes one line (newline appended), recording it in the transcript.
    pub async fn write_line(&self, line: String) -> Result<(), DriverError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(StdinCmd::Line(line, reply))
            .map_err(|_| DriverError::Closed)?;
        rx.await.map_err(|_| DriverError::Closed)?
    }

    /// Closes stdin (EOF); later writes fail with `Closed`.
    pub fn close(&self) {
        let _ = self.tx.send(StdinCmd::Close);
    }
}

async fn stdin_actor(
    mut stdin: Option<ChildStdin>,
    mut rx: mpsc::UnboundedReceiver<StdinCmd>,
    transcript: SharedTranscript,
    events: mpsc::UnboundedSender<HarnessEvent>,
    mut exit: ExitWatch,
) {
    // Ends when the process exited (dropping its event sender so the event
    // channel can close) or when every handle is gone.
    loop {
        let cmd = tokio::select! {
            c = rx.recv() => c,
            _ = exit.wait_for(|v| v.is_some()) => None,
        };
        let Some(cmd) = cmd else { return };
        match cmd {
            StdinCmd::Close => stdin = None,
            StdinCmd::Line(line, reply) => {
                let res = match stdin.as_mut() {
                    None => Err(DriverError::Closed),
                    Some(s) => {
                        let offset =
                            record(&transcript, Dir::Out, Channel::Stdin, &line, &[], &events);
                        let _ = events.send(HarnessEvent::Raw {
                            channel: Channel::Stdin,
                            line: line.clone(),
                            offset,
                        });
                        let mut bytes = line.into_bytes();
                        bytes.push(b'\n');
                        match s.write_all(&bytes).await {
                            Ok(()) => s.flush().await.map_err(DriverError::from),
                            Err(e) => Err(e.into()),
                        }
                    }
                };
                let _ = reply.send(res);
            }
        }
    }
}

/// Appends to the transcript; reports the first failure once as an `Error`.
pub fn record(
    transcript: &SharedTranscript,
    dir: Dir,
    channel: Channel,
    raw: &str,
    events: &[HarnessEvent],
    tx: &mpsc::UnboundedSender<HarnessEvent>,
) -> Option<u64> {
    record_with(transcript, dir, channel, raw, events, |e| {
        let _ = tx.send(e);
    })
}

/// [`record`] with a custom way to report the failure (e.g. an [`EventSink`]).
pub fn record_with(
    transcript: &SharedTranscript,
    dir: Dir,
    channel: Channel,
    raw: &str,
    events: &[HarnessEvent],
    report: impl FnOnce(HarnessEvent),
) -> Option<u64> {
    let mut w = transcript.lock().unwrap_or_else(|p| p.into_inner());
    match w.append(dir, channel, raw, events) {
        Ok(off) => Some(off),
        Err(e) => {
            if w.take_first_failure() {
                report(HarnessEvent::Error {
                    kind: so_core::ErrorKind::Other,
                    message: format!("transcript write failed: {e}"),
                });
            }
            None
        }
    }
}

/// A sender into the event stream that is shut at `Exited`: after the waiter
/// sent `Exited` every later [`EventSink::send`] is dropped, so `Exited` is
/// always the last event even for producers that outlive the process.
#[derive(Debug, Clone)]
pub struct EventSink(Arc<Mutex<Option<mpsc::UnboundedSender<HarnessEvent>>>>);

impl EventSink {
    /// False if the stream is already shut (the event was dropped).
    pub fn send(&self, e: HarnessEvent) -> bool {
        let g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        g.as_ref().is_some_and(|tx| tx.send(e).is_ok())
    }

    fn close_with(&self, last: HarnessEvent) {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = g.take() {
            let _ = tx.send(last);
        }
    }
}

/// Held by a driver task that still produces events after the process may
/// have exited. `Exited` is only sent once every guard is dropped (the waiter
/// gives up after 2 s and shuts the [`EventSink`] anyway).
#[derive(Debug)]
pub struct ProducerGuard(#[allow(dead_code)] mpsc::Sender<()>);

/// Reads one line of at most [`MAX_LINE_BYTES`] into `buf` (cleared first).
/// Returns `None` at EOF, else whether the line was truncated.
async fn read_capped<R: AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> std::io::Result<Option<bool>> {
    buf.clear();
    let mut truncated = false;
    let mut any = false;
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(any.then_some(truncated));
        }
        any = true;
        let nl = avail.iter().position(|b| *b == b'\n');
        let chunk_len = nl.unwrap_or(avail.len());
        let room = MAX_LINE_BYTES.saturating_sub(buf.len());
        let take = chunk_len.min(room);
        truncated |= take < chunk_len;
        buf.extend_from_slice(&avail[..take]);
        r.consume(chunk_len + usize::from(nl.is_some()));
        if nl.is_some() {
            return Ok(Some(truncated));
        }
    }
}

async fn pump<R: AsyncRead + Unpin>(
    reader: R,
    channel: Channel,
    mut parser: Option<Box<dyn LineParser>>,
    transcript: SharedTranscript,
    tx: mpsc::UnboundedSender<HarnessEvent>,
) {
    let dir = if channel == Channel::Stderr {
        Dir::Err
    } else {
        Dir::In
    };
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        match read_capped(&mut reader, &mut buf).await {
            Ok(None) => {
                if let Some(p) = parser.as_mut() {
                    p.on_eof();
                }
                return;
            }
            Ok(Some(truncated)) => {
                let mut line = String::from_utf8_lossy(&buf).into_owned();
                if line.ends_with('\r') {
                    line.pop();
                }
                let events = if truncated {
                    line.push_str(TRUNCATED_MARK);
                    match parser.as_mut() {
                        Some(p) => p.on_truncated(channel),
                        None => vec![default_truncated_event()],
                    }
                } else {
                    parser
                        .as_mut()
                        .map(|p| p.parse_line(&line))
                        .unwrap_or_default()
                };
                let offset = record(&transcript, dir, channel, &line, &events, &tx);
                let _ = tx.send(HarnessEvent::Raw {
                    channel,
                    line,
                    offset,
                });
                for e in events {
                    let _ = tx.send(e);
                }
            }
            Err(e) => {
                let _ = tx.send(HarnessEvent::Error {
                    kind: so_core::ErrorKind::Other,
                    message: format!("read error: {e}"),
                });
                if let Some(p) = parser.as_mut() {
                    p.on_eof();
                }
                // Keep the pipe drained so the child never blocks on it.
                let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
                return;
            }
        }
    }
}

/// Control over a supervised process.
#[derive(Debug)]
pub struct ProcessHandle {
    pub stdin: StdinHandle,
    pub guard: ProcessGuard,
    pub exit: ExitWatch,
    pub pid: Option<u32>,
    pub grace: Duration,
}

impl ProcessHandle {
    /// Closes stdin and waits up to `deadline` for a clean exit; otherwise
    /// kills the process group (SIGTERM, then SIGKILL after the grace). When
    /// the leader exits on its own, leftover group members are killed too.
    pub async fn shutdown(&self, deadline: Duration) -> ShutdownOutcome {
        self.stdin.close();
        if let Some(code) = exited(&self.exit, deadline).await {
            return ShutdownOutcome::Exited(code);
        }
        self.guard.terminate(self.grace).await;
        exited(&self.exit, Duration::from_secs(5)).await;
        ShutdownOutcome::Killed
    }

    /// For servers that never exit by themselves: closes stdin, waits `wait`
    /// for an exit, then SIGTERMs the group and waits the grace. `Exited` if
    /// that sufficed, `Killed` only after SIGKILL was needed.
    pub async fn shutdown_by_signal(&self, wait: Duration) -> ShutdownOutcome {
        self.stdin.close();
        if let Some(code) = exited(&self.exit, wait).await {
            return ShutdownOutcome::Exited(code);
        }
        self.guard.term();
        if let Some(code) = exited(&self.exit, self.grace).await {
            return ShutdownOutcome::Exited(code);
        }
        self.guard.terminate(Duration::ZERO).await;
        exited(&self.exit, Duration::from_secs(5)).await;
        ShutdownOutcome::Killed
    }
}

/// A started process: its event stream and its control handle.
#[derive(Debug)]
pub struct Supervised {
    pub events: mpsc::UnboundedReceiver<HarnessEvent>,
    /// The run's transcript, shared so a driver can append its own channels
    /// (opencode HTTP and SSE) with consistent offsets.
    pub transcript: SharedTranscript,
    pub handle: ProcessHandle,
    /// A sender into `events` that is shut at `Exited`.
    pub events_sink: EventSink,
    producers: mpsc::Sender<()>,
}

impl Supervised {
    /// A guard that delays `Exited` until it is dropped (see [`ProducerGuard`]).
    /// Take it before the process can exit.
    pub fn producer_guard(&self) -> ProducerGuard {
        ProducerGuard(self.producers.clone())
    }
}

/// Variables that point a harness at its run's temp dir.
const RUN_TMP_VARS: &[&str] = &["TMPDIR", "CLAUDE_CODE_TMPDIR", "BUN_TMPDIR"];

/// Spawns `cmd` as a supervised process: piped stdio, `kill_on_drop`, own
/// process group, transcript in `run_dir`. Events: `Raw` for every line on
/// stdout, stderr and stdin; parser events after the matching `Raw`; `Exited`
/// last and terminal (the event channel closes after it). The exit flag flips right after the leader is reaped, before any send.
pub fn spawn_supervised(
    cmd: Command,
    run_dir: &Path,
    grace: Duration,
    parser: impl LineParser,
    pass_env: &[String],
) -> Result<Supervised, DriverError> {
    spawn_supervised_with(cmd, run_dir, grace, parser, None, pass_env)
}

/// [`spawn_supervised`] with an optional parser for stderr as well. `pass_env` names inherited
/// variables the child may see beyond the baseline allowlist.
pub fn spawn_supervised_with(
    mut cmd: Command,
    run_dir: &Path,
    grace: Duration,
    parser: impl LineParser,
    stderr_parser: Option<Box<dyn LineParser>>,
    pass_env: &[String],
) -> Result<Supervised, DriverError> {
    let tmp = so_core::paths::run_tmp_dir(run_dir);
    create_private_dir(&tmp)
        .map_err(|e| DriverError::Other(format!("create {}: {e}", tmp.display())))?;
    for name in RUN_TMP_VARS {
        // An explicit account or driver value for the name wins.
        if !cmd.as_std().get_envs().any(|(k, _)| k == *name) {
            cmd.env(name, &tmp);
        }
    }
    // Every harness the supervisor launches starts from the isolated baseline
    // plus the account's opted-in names (GHSA-rfwj-83wf-qr2g). Explicit
    // driver and launch variables already set on `cmd` still win.
    so_core::child_env::isolate(cmd.as_std_mut(), pass_env);
    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let transcript: SharedTranscript =
        std::sync::Arc::new(Mutex::new(TranscriptWriter::create(run_dir)?));
    let mut child: Child = cmd
        .spawn()
        .map_err(|source| DriverError::Spawn { program, source })?;
    let pid = child.id();
    let (tx, rx) = mpsc::unbounded_channel();

    let readers: Vec<_> = [
        child.stdout.take().map(|o| {
            tokio::spawn(pump(
                o,
                Channel::Stdout,
                Some(Box::new(parser) as Box<dyn LineParser>),
                transcript.clone(),
                tx.clone(),
            ))
        }),
        child.stderr.take().map(|e| {
            tokio::spawn(pump(
                e,
                Channel::Stderr,
                stderr_parser,
                transcript.clone(),
                tx.clone(),
            ))
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let (exit_tx, exit_rx) = watch::channel(None);
    let (guard, group) = ProcessGuard::new(pid, kill_tx);

    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
    tokio::spawn(stdin_actor(
        child.stdin.take(),
        stdin_rx,
        transcript.clone(),
        tx.clone(),
        exit_rx.clone(),
    ));
    let sink = EventSink(Arc::new(Mutex::new(Some(tx.clone()))));
    let (producers_tx, mut producers_rx) = mpsc::channel::<()>(1);
    let waiter_sink = sink.clone();

    tokio::spawn(async move {
        let status = tokio::select! {
            s = child.wait() => s,
            _ = kill_rx => {
                let _ = child.kill().await;
                child.wait().await
            }
        };
        let code = status.ok().and_then(|s| s.code());
        let _ = exit_tx.send(Some(code));
        // Leftover group members would keep the pipes open: kill them now.
        group.kill_leftovers_and_disarm();
        for mut r in readers {
            // A process that escaped the group may still hold the pipes.
            if tokio::time::timeout(Duration::from_secs(2), &mut r)
                .await
                .is_err()
            {
                r.abort();
                let _ = r.await;
            }
        }
        drop(tx);
        // Wait for producer guards (bounded), then send `Exited` and shut the
        // sink: after Exited no sender is left, so the channel closes.
        let _ = tokio::time::timeout(Duration::from_secs(2), producers_rx.recv()).await;
        waiter_sink.close_with(HarnessEvent::Exited { code });
    });

    Ok(Supervised {
        events: rx,
        transcript,
        handle: ProcessHandle {
            stdin: StdinHandle { tx: stdin_tx },
            guard,
            exit: exit_rx,
            pid,
            grace,
        },
        events_sink: sink,
        producers: producers_tx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_tightens_existing_files() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        std::fs::write(&f, "old old old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&f, b"new").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    struct NoParse;
    impl LineParser for NoParse {
        fn parse_line(&mut self, _: &str) -> Vec<HarnessEvent> {
            vec![]
        }
    }

    async fn stdout_lines(cmd: Command, run_dir: &Path) -> Vec<String> {
        let mut s = spawn_supervised(cmd, run_dir, Duration::from_secs(1), NoParse, &[]).unwrap();
        let mut out = vec![];
        while let Some(ev) = s.events.recv().await {
            if let HarnessEvent::Raw {
                channel: Channel::Stdout,
                line,
                ..
            } = ev
            {
                out.push(line);
            }
        }
        out
    }

    const PROBE: &str =
        r#"echo "$TMPDIR"; echo "$CLAUDE_CODE_TMPDIR"; echo "$BUN_TMPDIR"; stat -c %a "$TMPDIR""#;

    #[tokio::test]
    async fn child_gets_a_private_run_tmp_dir() {
        let d = tempfile::tempdir().unwrap();
        let run_dir = d.path().join("run");
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", PROBE]);
        let lines = stdout_lines(cmd, &run_dir).await;
        let tmp = so_core::paths::run_tmp_dir(&run_dir).display().to_string();
        assert_eq!(lines, [tmp.clone(), tmp.clone(), tmp, "700".to_string()]);
    }

    #[tokio::test]
    async fn explicit_tmpdir_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let run_dir = d.path().join("run");
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", r#"echo "$TMPDIR"; echo "$BUN_TMPDIR""#])
            .env("TMPDIR", "/explicit");
        let lines = stdout_lines(cmd, &run_dir).await;
        let tmp = so_core::paths::run_tmp_dir(&run_dir).display().to_string();
        assert_eq!(lines, ["/explicit".to_string(), tmp]);
    }
}
