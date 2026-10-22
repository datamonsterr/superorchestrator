//! Single-instance lock: an exclusive `flock` on `<config_dir>/supervisor.lock`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::error::{Result, SupervisorError};

/// Holds the lock for as long as it lives; dropping it closes the file and releases the lock.
#[derive(Debug)]
pub struct SupervisorLock {
    _file: File,
    path: PathBuf,
}

impl SupervisorLock {
    pub fn lock_path(config_dir: &Path) -> PathBuf {
        config_dir.join("supervisor.lock")
    }

    /// Takes the lock without blocking and writes our pid into the file.
    pub fn acquire(config_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(config_dir)?;
        let path = Self::lock_path(config_dir);
        // No truncation before the lock is held: the holder's pid must stay readable.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => {
                let mut s = String::new();
                let _ = file.read_to_string(&mut s);
                return Err(SupervisorError::AlreadyRunning {
                    pid: s.trim().parse().ok(),
                });
            }
            Err(e) => return Err(std::io::Error::from(e).into()),
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_then_succeeds_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = SupervisorLock::acquire(dir.path()).unwrap();
        match SupervisorLock::acquire(dir.path()) {
            Err(SupervisorError::AlreadyRunning { pid }) => {
                assert_eq!(pid, Some(std::process::id()));
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
        // The failed attempt must not have clobbered the pid.
        let content = std::fs::read_to_string(first.path()).unwrap();
        assert_eq!(content.trim(), std::process::id().to_string());
        drop(first);
        SupervisorLock::acquire(dir.path()).unwrap();
    }
}
