//! A size-rotated log file for the detached supervisor (SPEC 16): `supervisor.log` is
//! renamed to `.1` when it would pass the limit, `.1` to `.2` and so on; the oldest
//! (`.keep`) is dropped. All files stay mode 0600.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Size at which `supervisor.log` rotates.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Rotated files kept next to the live one (`supervisor.log.1` ..= `.5`).
pub const KEEP: usize = 5;

struct Inner {
    path: PathBuf,
    max: u64,
    keep: usize,
    file: File,
    len: u64,
    /// When the last rotation failed: no retry before [`RETRY_AFTER`] has passed, so a
    /// persistent failure does not shift the kept history away on every write.
    failed_at: Option<Instant>,
}

const RETRY_AFTER: Duration = Duration::from_secs(60);

/// Cheap to clone; every clone writes to the same file.
#[derive(Clone)]
pub struct RotatingLog(Arc<Mutex<Inner>>);

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

fn rotated(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl RotatingLog {
    pub fn open(path: &Path, max: u64, keep: usize) -> io::Result<Self> {
        let file = open_append(path)?;
        let len = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(Inner {
            path: path.to_path_buf(),
            max,
            keep: keep.max(1),
            file,
            len,
            failed_at: None,
        }))))
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        let _ = std::fs::remove_file(rotated(&self.path, self.keep));
        for n in (1..self.keep).rev() {
            let from = rotated(&self.path, n);
            if from.exists() {
                std::fs::rename(from, rotated(&self.path, n + 1))?;
            }
        }
        std::fs::rename(&self.path, rotated(&self.path, 1))?;
        self.file = open_append(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    /// Each call is written whole, so a rotation never tears a chunk. The redacting
    /// writer in front may still flush an oversize line in several chunks, and a rotation
    /// can fall between them; lines are normally flushed whole.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut g = self.inner();
        let retry_ok = g.failed_at.is_none_or(|t| t.elapsed() >= RETRY_AFTER);
        if g.len > 0 && g.len + buf.len() as u64 > g.max && retry_ok {
            // A failed rotation must not lose the log line: keep appending to the old file.
            g.failed_at = g.rotate().err().map(|_| Instant::now());
        }
        g.file.write_all(buf)?;
        g.len += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner().file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn rotates_by_size_and_keeps_a_fixed_number_of_files() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("supervisor.log");
        let mut log = RotatingLog::open(&path, 20, 3).unwrap();
        for i in 0..10 {
            log.write_all(format!("line-{i:02}-xxxxxx\n").as_bytes())
                .unwrap();
        }
        // 16 bytes a line, 20 per file: one line per file.
        assert_eq!(read(&path), "line-09-xxxxxx\n");
        assert_eq!(read(&rotated(&path, 1)), "line-08-xxxxxx\n");
        assert_eq!(read(&rotated(&path, 2)), "line-07-xxxxxx\n");
        assert_eq!(read(&rotated(&path, 3)), "line-06-xxxxxx\n");
        assert!(!rotated(&path, 4).exists(), "only `keep` files are kept");
        for f in [
            path.clone(),
            rotated(&path, 1),
            rotated(&path, 2),
            rotated(&path, 3),
        ] {
            assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn resumes_the_size_of_an_existing_file_and_clones_share_it() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("supervisor.log");
        std::fs::write(&path, "0123456789012345").unwrap();
        let log = RotatingLog::open(&path, 20, 2).unwrap();
        let (mut a, mut b) = (log.clone(), log);
        a.write_all(b"abcd").unwrap();
        b.write_all(b"efgh").unwrap();
        assert_eq!(read(&rotated(&path, 1)), "0123456789012345abcd");
        assert_eq!(read(&path), "efgh");
    }

    #[test]
    fn a_failed_rotation_keeps_writing_and_backs_off() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("supervisor.log");
        let mut log = RotatingLog::open(&path, 10, 1).unwrap();
        // A non-empty directory in the way of `.1` makes the rename fail.
        let blocker = rotated(&path, 1);
        std::fs::create_dir_all(blocker.join("x")).unwrap();
        log.write_all(b"aaaaaaaaaa").unwrap();
        log.write_all(b"bbbbbbbbbb").unwrap();
        assert!(blocker.is_dir(), "the failed rotation changed nothing else");
        assert_eq!(read(&path), "aaaaaaaaaabbbbbbbbbb");
        // Within the back-off no second attempt is made, even once the way is clear.
        std::fs::remove_dir_all(&blocker).unwrap();
        log.write_all(b"cccccccccc").unwrap();
        assert!(!blocker.exists());
        // After it, rotation resumes.
        log.inner().failed_at = Some(Instant::now() - RETRY_AFTER);
        log.write_all(b"d").unwrap();
        assert_eq!(read(&blocker), "aaaaaaaaaabbbbbbbbbbcccccccccc");
        assert_eq!(read(&path), "d");
    }

    #[test]
    fn a_line_larger_than_the_limit_is_still_written() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("supervisor.log");
        let mut log = RotatingLog::open(&path, 4, 2).unwrap();
        log.write_all(b"a very long line\n").unwrap();
        assert_eq!(read(&path), "a very long line\n");
    }
}
