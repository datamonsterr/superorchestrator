//! Wrapper scripts (SPEC 5.3).

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{AccountsError, Result};

const MARKER: &str = "# managed-by: superorchestrator";

/// True when the file exists and carries the ownership marker line.
pub fn is_managed(path: &Path) -> bool {
    std::fs::read(path)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .lines()
                .any(|l| l.trim_end() == MARKER)
        })
        .unwrap_or(false)
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Writes `<dir>/<account_name>` atomically with mode 0755. A file that exists
/// without the marker is first moved to `<bak_dir>/<name>.<timestamp>`.
/// Returns the wrapper path.
pub fn write_wrapper(
    dir: &Path,
    bak_dir: &Path,
    account_name: &str,
    self_exe_name: &str,
) -> Result<PathBuf> {
    let path = dir.join(account_name);
    std::fs::create_dir_all(dir).map_err(|e| AccountsError::io(dir, e))?;
    if path.symlink_metadata().is_ok() && !is_managed(&path) {
        std::fs::create_dir_all(bak_dir).map_err(|e| AccountsError::io(bak_dir, e))?;
        let bak = bak_dir.join(format!("{account_name}.{}", timestamp()));
        if std::fs::rename(&path, &bak).is_err() {
            std::fs::copy(&path, &bak).map_err(|e| AccountsError::io(&bak, e))?;
            std::fs::remove_file(&path).map_err(|e| AccountsError::io(&path, e))?;
        }
    }
    let body = format!("#!/bin/sh\n{MARKER}\nexec {self_exe_name} exec {account_name} -- \"$@\"\n");
    let tmp = dir.join(format!(".{account_name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        // `mode` is masked by umask; make the bits exact.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(AccountsError::io(&path, e));
    }
    Ok(path)
}

/// Removes `<dir>/<name>` only when it carries the marker. Returns whether a
/// file was removed.
pub fn remove_wrapper(dir: &Path, account_name: &str) -> Result<bool> {
    let path = dir.join(account_name);
    if !is_managed(&path) {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| AccountsError::io(&path, e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_managed_executable_wrapper() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let bak = d.path().join("bak");
        let p = write_wrapper(&bin, &bak, "codex-acc2", "superorchestrator").unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "#!/bin/sh\n# managed-by: superorchestrator\nexec superorchestrator exec codex-acc2 -- \"$@\"\n"
        );
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(is_managed(&p));
        // Rewriting a managed wrapper makes no backup and leaves no temp file.
        write_wrapper(&bin, &bak, "codex-acc2", "superorchestrator").unwrap();
        assert!(!bak.exists());
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
    }

    #[test]
    fn foreign_file_is_backed_up() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let bak = d.path().join("bak");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex-acc2"), "#!/bin/sh\necho old\n").unwrap();
        let p = write_wrapper(&bin, &bak, "codex-acc2", "superorchestrator").unwrap();
        assert!(is_managed(&p));
        let baks: Vec<_> = std::fs::read_dir(&bak).unwrap().collect();
        assert_eq!(baks.len(), 1);
        let b = baks[0].as_ref().unwrap();
        assert!(b.file_name().to_string_lossy().starts_with("codex-acc2."));
        assert_eq!(
            std::fs::read_to_string(b.path()).unwrap(),
            "#!/bin/sh\necho old\n"
        );
    }

    #[test]
    fn remove_only_managed() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let bak = d.path().join("bak");
        write_wrapper(&bin, &bak, "a-one", "superorchestrator").unwrap();
        std::fs::write(bin.join("b-two"), "#!/bin/sh\n").unwrap();
        assert!(remove_wrapper(&bin, "a-one").unwrap());
        assert!(!bin.join("a-one").exists());
        assert!(!remove_wrapper(&bin, "b-two").unwrap());
        assert!(bin.join("b-two").exists());
        assert!(!remove_wrapper(&bin, "missing").unwrap());
        assert!(!is_managed(&bin.join("missing")));
    }
}
