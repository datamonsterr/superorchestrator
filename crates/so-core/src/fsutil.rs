//! Small filesystem helpers.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{CoreError, Result};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` atomically: a mode-0600 temp file with a unique name is
/// created in the same directory (`create_new`), written, fsynced and renamed over the
/// target, then the directory is fsynced (errors ignored where a directory cannot be opened).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{n}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::io(path, e));
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Creates `dir` and its missing parents with mode 0700.
pub fn create_private_dirs(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Writes `bytes` over `path` atomically for files the user may own: a symlink is followed
/// (the link survives), missing parent directories are created private (0700), the file
/// keeps its mode (0600 when new), and a temp file next to the target (`create_new`) is
/// fsynced and renamed into place.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    create_private_dirs(dir)?;
    let mode = std::fs::metadata(&target).map_or(0o600, |m| m.mode() & 0o7777);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{n}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        // The umask may have cut the mode at creation.
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_atomic_creates_private_dirs_and_files_and_keeps_mode_and_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a/b/f.md");
        write_file_atomic(&p, b"one").unwrap();
        let mode = |q: &Path| std::fs::metadata(q).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = d.path().join("link.md");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        write_file_atomic(&link, b"two").unwrap();
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(mode(&p), 0o640);
        let left: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn writes_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.toml");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_write_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        // The target is a non-empty directory, so the final rename fails.
        let target = d.path().join("t.toml");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("x"), "x").unwrap();
        assert!(write_atomic(&target, b"data").is_err());
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("t.toml")]);
    }
}
