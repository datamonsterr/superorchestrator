//! Notifications (SPEC 18): Discord webhook and desktop, for the events in
//! `notifications.on`, rate limited to one per run per event type per window.
//!
//! A notification carries only the event name, the run name (or id) and a short reason.
//! Never task text, results or secrets. The webhook URL is a secret: it is read from the
//! environment variable the config names and never logged.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use so_core::config::Notifications as NotifyConfig;

/// One notification per run per event type in this window.
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600);
const MAX_REASON_CHARS: usize = 120;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub type SinkFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// A place a notification goes to. Failures are the sink's to log; they never fail a run.
pub trait Sink: Send + Sync {
    fn send<'a>(&'a self, title: &'a str, body: &'a str) -> SinkFuture<'a>;
}

/// Filters, rate limits and fans a notification out to the sinks.
pub struct Notifications {
    on: HashSet<String>,
    window: Duration,
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
    last: Mutex<HashMap<(String, String), Instant>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn short(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_REASON_CHARS {
        return flat;
    }
    let mut s: String = flat.chars().take(MAX_REASON_CHARS - 1).collect();
    s.push('…');
    s
}

impl Notifications {
    /// No sinks: nothing is sent until [`Notifications::set_sinks`].
    pub fn new(on: &[String]) -> Self {
        Self {
            on: on.iter().cloned().collect(),
            window: RATE_LIMIT_WINDOW,
            sinks: Mutex::default(),
            last: Mutex::default(),
        }
    }

    #[must_use]
    pub fn with_window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }

    pub fn set_sinks(&self, sinks: Vec<Arc<dyn Sink>>) {
        *lock(&self.sinks) = sinks;
    }

    /// Sends `event` about `key` (a run or session id) unless it is not enabled, there is
    /// no sink, or the same event was already sent for `key` within the window.
    /// Returns whether it was sent.
    pub async fn notify(&self, event: &str, key: &str, name: &str, reason: &str) -> bool {
        if !self.on.contains(event) {
            return false;
        }
        let sinks: Vec<Arc<dyn Sink>> = lock(&self.sinks).clone();
        if sinks.is_empty() {
            return false;
        }
        {
            let now = Instant::now();
            let mut last = lock(&self.last);
            let k = (key.to_string(), event.to_string());
            if last
                .get(&k)
                .is_some_and(|t| now.duration_since(*t) < self.window)
            {
                return false;
            }
            last.retain(|_, t| now.duration_since(*t) < self.window);
            last.insert(k, now);
        }
        let title = format!("superorchestrator: {event}");
        let reason = short(reason);
        let body = if reason.is_empty() {
            short(name)
        } else {
            format!("{}: {reason}", short(name))
        };
        for s in sinks {
            s.send(&title, &body).await;
        }
        true
    }
}

/// Posts `{"content": ...}` to a Discord webhook.
pub struct DiscordSink {
    url: String,
    client: reqwest::Client,
}

impl DiscordSink {
    pub fn new(url: String, client: reqwest::Client) -> Self {
        Self { url, client }
    }
}

impl Sink for DiscordSink {
    fn send<'a>(&'a self, title: &'a str, body: &'a str) -> SinkFuture<'a> {
        Box::pin(async move {
            let content = format!("**{title}**\n{body}");
            let r = self
                .client
                .post(&self.url)
                .timeout(SEND_TIMEOUT)
                .json(&serde_json::json!({ "content": content }))
                .send()
                .await;
            match r {
                // The error text would contain the webhook URL: drop it.
                Err(e) => tracing::warn!(error = %e.without_url(), "discord notification failed"),
                Ok(resp) if !resp.status().is_success() => {
                    tracing::warn!(status = %resp.status(), "discord notification rejected");
                }
                Ok(_) => {}
            }
        })
    }
}

/// `notify-send` on Linux, `osascript` on macOS. Arguments go in argv, never through a
/// shell, so the text cannot inject commands.
pub struct DesktopSink;

/// The program and arguments that show a desktop notification, or `None` on a platform
/// without one.
pub fn desktop_command(title: &str, body: &str) -> Option<(&'static str, Vec<String>)> {
    if cfg!(target_os = "linux") {
        Some((
            "notify-send",
            vec![
                "--app-name=superorchestrator".into(),
                "--".into(),
                title.into(),
                body.into(),
            ],
        ))
    } else if cfg!(target_os = "macos") {
        // The text is passed as script arguments, not spliced into the script.
        Some((
            "osascript",
            vec![
                "-e".into(),
                "on run argv".into(),
                "-e".into(),
                "display notification (item 2 of argv) with title (item 1 of argv)".into(),
                "-e".into(),
                "end run".into(),
                title.into(),
                body.into(),
            ],
        ))
    } else {
        None
    }
}

impl Sink for DesktopSink {
    fn send<'a>(&'a self, title: &'a str, body: &'a str) -> SinkFuture<'a> {
        Box::pin(async move {
            let Some((program, args)) = desktop_command(title, body) else {
                return;
            };
            let run = tokio::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match tokio::time::timeout(SEND_TIMEOUT, run).await {
                Ok(Ok(st)) if st.success() => {}
                Ok(Ok(st)) => tracing::debug!(%st, program, "desktop notification failed"),
                Ok(Err(e)) => tracing::debug!(error = %e, program, "desktop notification failed"),
                Err(_) => tracing::debug!(program, "desktop notification timed out"),
            }
        })
    }
}

/// The sinks `cfg` asks for: Discord when the named variable holds a URL, and the desktop
/// when enabled.
pub fn sinks_from_config(cfg: &NotifyConfig, client: &reqwest::Client) -> Vec<Arc<dyn Sink>> {
    let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
    if let Some(url) = std::env::var(&cfg.discord_webhook_env)
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        sinks.push(Arc::new(DiscordSink::new(url, client.clone())));
    }
    if cfg.desktop {
        sinks.push(Arc::new(DesktopSink));
    }
    sinks
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Default)]
    pub struct Recorder {
        pub sent: Mutex<Vec<(String, String)>>,
    }

    impl Sink for Recorder {
        fn send<'a>(&'a self, title: &'a str, body: &'a str) -> SinkFuture<'a> {
            Box::pin(async move {
                lock(&self.sent).push((title.to_string(), body.to_string()));
            })
        }
    }

    fn setup(window: Duration) -> (Notifications, Arc<Recorder>) {
        let n = Notifications::new(&["escalated".to_string(), "session_finished".to_string()])
            .with_window(window);
        let rec = Arc::new(Recorder::default());
        n.set_sinks(vec![rec.clone()]);
        (n, rec)
    }

    #[tokio::test]
    async fn one_per_run_per_event_per_window() {
        let (n, rec) = setup(Duration::from_secs(600));
        assert!(n.notify("escalated", "r1", "build", "stalled").await);
        assert!(!n.notify("escalated", "r1", "build", "stalled").await);
        // Another run and another event type are separate limits.
        assert!(n.notify("escalated", "r2", "docs", "stalled").await);
        assert!(n.notify("session_finished", "r1", "build", "").await);
        assert_eq!(lock(&rec.sent).len(), 3);
    }

    #[tokio::test]
    async fn window_expires() {
        let (n, rec) = setup(Duration::from_millis(30));
        assert!(n.notify("escalated", "r1", "a", "x").await);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(n.notify("escalated", "r1", "a", "x").await);
        assert_eq!(lock(&rec.sent).len(), 2);
    }

    #[tokio::test]
    async fn only_listed_events_and_only_with_sinks() {
        let (n, rec) = setup(Duration::from_secs(600));
        assert!(!n.notify("account_exhausted", "a", "a", "x").await);
        assert!(lock(&rec.sent).is_empty());
        let none = Notifications::new(&["escalated".to_string()]);
        assert!(!none.notify("escalated", "r", "n", "x").await);
        // A refused send does not use up the window.
        none.set_sinks(vec![rec.clone()]);
        assert!(none.notify("escalated", "r", "n", "x").await);
    }

    #[tokio::test]
    async fn text_is_short_and_flat() {
        let (n, rec) = setup(Duration::from_secs(600));
        let long = "word ".repeat(100);
        n.notify("escalated", "r", "name", &long).await;
        let sent = lock(&rec.sent);
        assert_eq!(sent[0].0, "superorchestrator: escalated");
        assert!(sent[0].1.starts_with("name: word word"));
        assert!(sent[0].1.chars().count() <= "name: ".len() + MAX_REASON_CHARS);
        assert!(sent[0].1.ends_with('…'));
    }

    #[test]
    fn desktop_text_never_reaches_a_shell() {
        let Some((program, args)) = desktop_command("t; rm -rf /", "$(x)") else {
            return;
        };
        assert!(program == "notify-send" || program == "osascript");
        assert!(args.iter().any(|a| a == "t; rm -rf /"));
        assert!(args.iter().all(|a| a != "sh" && a != "-c"));
    }
}
