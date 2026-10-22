//! `<run_dir>/transcript.jsonl` writer (SPEC 12).
//!
//! One line per harness line: `{"ts", "dir": "in"|"out"|"err", "raw": <line>, "event": ...}`.
//! `event` is the normalised event (one), an array (several) or `null`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::driver::HarnessEvent;

/// Direction of a transcript line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Read from the harness stdout.
    In,
    /// Written to the harness stdin.
    Out,
    /// Read from the harness stderr.
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

/// Append-only transcript writer that tracks byte offsets.
#[derive(Debug)]
pub struct TranscriptWriter {
    file: File,
    offset: u64,
}

impl TranscriptWriter {
    /// Path of the transcript inside a run directory.
    pub fn path(run_dir: &Path) -> PathBuf {
        run_dir.join("transcript.jsonl")
    }

    /// Opens (creating) the transcript for append.
    pub fn create(run_dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(run_dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::path(run_dir))?;
        let offset = file.metadata()?.len();
        Ok(Self { file, offset })
    }

    /// Appends one entry and returns the byte offset where it starts.
    pub fn append(&mut self, dir: Dir, raw: &str, events: &[HarnessEvent]) -> io::Result<u64> {
        let event = match events {
            [] => serde_json::Value::Null,
            [one] => json!(one),
            many => json!(many),
        };
        let entry = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "dir": dir.as_str(),
            "raw": raw,
            "event": event,
        });
        let mut line = entry.to_string();
        line.push('\n');
        let start = self.offset;
        self.file.write_all(line.as_bytes())?;
        self.offset += line.len() as u64;
        Ok(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_point_at_entries() {
        let d = tempfile::tempdir().unwrap();
        let mut w = TranscriptWriter::create(d.path()).unwrap();
        let a = w.append(Dir::Out, "hello", &[]).unwrap();
        let b = w
            .append(
                Dir::In,
                "x",
                &[HarnessEvent::AssistantMessage { text: "t".into() }],
            )
            .unwrap();
        assert_eq!(a, 0);
        let text = std::fs::read_to_string(TranscriptWriter::path(d.path())).unwrap();
        let tail: serde_json::Value = serde_json::from_str(text[b as usize..].trim()).unwrap();
        assert_eq!(tail["dir"], "in");
        assert_eq!(tail["event"]["type"], "assistant_message");
        let w2 = TranscriptWriter::create(d.path()).unwrap();
        assert_eq!(w2.offset, text.len() as u64);
    }
}
