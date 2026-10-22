//! Wrapper scripts (SPEC 5.3).

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use so_core::validate_account_name;

use crate::error::{AccountsError, Result};

const MARKER: &str = "# managed-by: superorchestrator";

/// True when `path` is a regular file (not a symlink) whose second line is the
/// ownership marker. Only the first 256 bytes are examined.
pub fn is_managed(path: &Path) -> bool {
    use std::io::Read;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = Vec::new();
    if f.take(256).read_to_end(&mut buf).is_err() {
        return false;
    }
    String::from_utf8_lossy(&buf).lines().nth(1) == Some(MARKER)
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn check_name(name: &str) -> Result<()> {
    validate_account_name(name)?;
    Ok(())
}

/// Reserves a fresh backup name with `create_new`, adding a counter on clash.
fn reserve_backup(bak_dir: &Path, name: &str) -> Result<PathBuf> {
    let ts = timestamp();
    for n in 0u32.. {
        let file = if n == 0 {
            format!("{name}.{ts}")
        } else {
            format!("{name}.{ts}.{n}")
        };
        let p = bak_dir.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(AccountsError::io(&p, e)),
        }
    }
    unreachable!("counter is unbounded")
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
    check_name(account_name)?;
    let path = dir.join(account_name);
    std::fs::create_dir_all(dir).map_err(|e| AccountsError::io(dir, e))?;
    if path.symlink_metadata().is_ok() && !is_managed(&path) {
        std::fs::create_dir_all(bak_dir).map_err(|e| AccountsError::io(bak_dir, e))?;
        let bak = reserve_backup(bak_dir, account_name)?;
        if std::fs::rename(&path, &bak).is_err() {
            std::fs::copy(&path, &bak).map_err(|e| AccountsError::io(&bak, e))?;
            std::fs::remove_file(&path).map_err(|e| AccountsError::io(&path, e))?;
        }
    }
    let body = format!("#!/bin/sh\n{MARKER}\nexec {self_exe_name} exec {account_name} -- \"$@\"\n");
    let mut n = 0u32;
    let (tmp, mut f) = loop {
        let tmp = dir.join(format!(".{account_name}.tmp.{}.{n}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&tmp)
        {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(AccountsError::io(&tmp, e)),
        }
    };
    let result = (|| {
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
    check_name(account_name)?;
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

    #[test]
    fn rejects_bad_names_and_symlink_or_late_marker() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let bak = d.path().join("bak");
        for bad in ["../evil", "codex", "A b", "a/b"] {
            assert!(
                write_wrapper(&bin, &bak, bad, "superorchestrator").is_err(),
                "{bad}"
            );
            assert!(remove_wrapper(&bin, bad).is_err(), "{bad}");
        }
        // Marker on line 3, or beyond 256 bytes, is not ownership.
        let f = d.path().join("f");
        std::fs::write(&f, "#!/bin/sh\n\n# managed-by: superorchestrator\n").unwrap();
        assert!(!is_managed(&f));
        std::fs::write(
            &f,
            format!(
                "#!/bin/sh\n{}\n# managed-by: superorchestrator\n",
                "x".repeat(300)
            ),
        )
        .unwrap();
        assert!(!is_managed(&f));
        // A symlink to a managed file is not managed.
        let p = write_wrapper(&bin, &bak, "a-one", "superorchestrator").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(!is_managed(&link));
    }

    #[test]
    fn backup_names_do_not_clash() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let bak = d.path().join("bak");
        std::fs::create_dir_all(&bin).unwrap();
        for i in 0..3 {
            std::fs::write(bin.join("a-one"), format!("old{i}")).unwrap();
            write_wrapper(&bin, &bak, "a-one", "superorchestrator").unwrap();
            std::fs::remove_file(bin.join("a-one")).unwrap();
        }
        assert_eq!(std::fs::read_dir(&bak).unwrap().count(), 3);
    }
}
