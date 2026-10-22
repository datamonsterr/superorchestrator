#![allow(dead_code)]

use std::path::PathBuf;

/// A binary of this workspace's `debug` profile, found next to the running test.
///
/// Test executables live in `<target>/debug/deps`, so the binary sits in the
/// grandparent directory. This works for any target dir, shared or per-checkout.
pub fn sibling_bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    debug.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
