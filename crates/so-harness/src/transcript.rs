//! `<run_dir>/transcript.jsonl` writer (SPEC 12).
//!
//! One line per harness line:
//! `{"ts", "dir": "in"|"out"|"err", "channel", "raw": <line>, "event": ...}`.
//! `event` is the normalised event (one), an array (several) or `null`.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::driver::{Channel, HarnessEvent};
use crate::process::create_private_dir;

/// Direction of a transcript line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Received from the harness.
    In,
    /// Sent to the harness.
    Out,
    /// Harness stderr.
    Err,
}

impl Dir {
    fn as_str(self) -> &'static str {
        match self {
            Dir::In => "in",
            Dir::Out => "out",
            Dir::Err => "err",
        }
    }
}

/// Transcript shared between the reader, stdin and waiter tasks.
pub type SharedTranscript = Arc<Mutex<TranscriptWriter>>;

/// Append-only transcript writer that tracks the real file position. After a
/// failed write it is poisoned: later appends fail without touching the file,
/// so offsets never point at a corrupt tail.
#[derive(Debug)]
pub struct TranscriptWriter {
    file: File,
    offset: u64,
    poisoned: bool,
    failure_reported: bool,
}

impl TranscriptWriter {
    /// Path of the transcript inside a run directory.
    pub fn path(run_dir: &Path) -> PathBuf {
        run_dir.join("transcript.jsonl")
    }

    /// Opens (creating, mode 0600) the transcript for append.
    pub fn create(run_dir: &Path) -> io::Result<Self> {
        create_private_dir(run_dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(Self::path(run_dir))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Self::from_file(file)
    }

    pub(crate) fn from_file(mut file: File) -> io::Result<Self> {
        let offset = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            offset,
            poisoned: false,
            failure_reported: false,
        })
    }

    /// Appends one entry and returns the byte offset where it starts.
    pub fn append(
        &mut self,
        dir: Dir,
        channel: Channel,
        raw: &str,
        events: &[HarnessEvent],
    ) -> io::Result<u64> {
        if self.poisoned {
            return Err(io::Error::other(
                "transcript is poisoned by an earlier write failure",
            ));
        }
        let event = match events {
            [] => serde_json::Value::Null,
            [one] => json!(one),
            many => json!(many),
        };
        let entry = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "dir": dir.as_str(),
            "channel": channel,
            "raw": raw,
            "event": event,
        });
        let mut line = entry.to_string();
        line.push('\n');
        let start = self.offset;
        if let Err(e) = self.file.write_all(line.as_bytes()) {
            self.poisoned = true;
            return Err(e);
        }
        self.offset += line.len() as u64;
        Ok(start)
    }

    /// True exactly once after the first failure, so callers report it once.
    pub fn take_first_failure(&mut self) -> bool {
        let first = self.poisoned && !self.failure_reported;
        self.failure_reported |= self.poisoned;
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_point_at_entries_and_mode_is_private() {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join("run");
        let mut w = TranscriptWriter::create(&run).unwrap();
        let a = w.append(Dir::Out, Channel::Stdin, "hello", &[]).unwrap();
        let b = w
            .append(
                Dir::In,
                Channel::Stdout,
                "x",
                &[HarnessEvent::AssistantMessage { text: "t".into() }],
            )
            .unwrap();
        assert_eq!(a, 0);
        let text = std::fs::read_to_string(TranscriptWriter::path(&run)).unwrap();
        let tail: serde_json::Value = serde_json::from_str(text[b as usize..].trim()).unwrap();
        assert_eq!(tail["dir"], "in");
        assert_eq!(tail["channel"], "stdout");
        assert_eq!(tail["event"]["type"], "assistant_message");
        let w2 = TranscriptWriter::create(&run).unwrap();
        assert_eq!(w2.offset, text.len() as u64);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&TranscriptWriter::path(&run)), 0o600);
        assert_eq!(mode(&run), 0o700);
    }

    #[test]
    fn create_tightens_existing_transcript() {
        let d = tempfile::tempdir().unwrap();
        let f = TranscriptWriter::path(d.path());
        std::fs::write(&f, "").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        TranscriptWriter::create(d.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_write_poisons_and_reports_once() {
        let f = OpenOptions::new().write(true).open("/dev/full").unwrap();
        let mut w = TranscriptWriter::from_file(f).unwrap();
        assert!(w.append(Dir::In, Channel::Stdout, "a", &[]).is_err());
        assert!(w.take_first_failure());
        assert!(!w.take_first_failure());
        let e = w.append(Dir::In, Channel::Stdout, "b", &[]).unwrap_err();
        assert!(e.to_string().contains("poisoned"));
    }
}
