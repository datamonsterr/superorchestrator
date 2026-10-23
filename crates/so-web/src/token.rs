//! The persistent UI token, shared by supervisor startup and the local CLI URL.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use rustix::fs::{Mode, OFlags};

/// 256 bits from the OS CSPRNG, encoded without URL or shell metacharacters.
pub(crate) fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn read_private(path: &Path) -> io::Result<String> {
    // Never follow a symlink or block on an attacker-supplied FIFO/device.
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ui-token must be an owner-only regular file (0600), owned by the current user",
        ));
    }
    let mut token = String::new();
    file.take(65).read_to_string(&mut token)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ui-token must contain exactly 64 hex characters; remove the invalid file to regenerate it",
        ));
    }
    Ok(token)
}

pub(crate) fn load_or_create(config_dir: &Path) -> io::Result<String> {
    let path = config_dir.join("ui-token");
    match read_private(&path) {
        Ok(token) => return Ok(token),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    so_core::fsutil::create_private_dirs(config_dir)?;
    let token = random_token()?;
    let mut temp = tempfile::NamedTempFile::new_in(config_dir)?;
    temp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    temp.write_all(token.as_bytes())?;
    temp.as_file().sync_all()?;
    // Publish a fully written file without replacing a concurrent startup's token.
    match temp.persist_noclobber(&path) {
        Ok(_) => {
            File::open(config_dir)?.sync_all()?;
            Ok(token)
        }
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => read_private(&path),
        Err(e) => Err(e.error),
    }
}

pub(crate) fn fragment(token: &str) -> String {
    token
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
