//! Process exit codes and the typed error that carries one.

use std::fmt;

pub const FAILURE: u8 = 1;
/// Reserved for clap usage errors (clap exits with it by itself).
#[allow(dead_code)]
pub const USAGE: u8 = 2;
pub const NOT_FOUND: u8 = 3;
pub const NOT_IMPLEMENTED: u8 = 4;

/// An error that maps to a specific exit code in `main`.
#[derive(Debug)]
pub struct CliError {
    pub code: u8,
    pub msg: String,
}

impl CliError {
    pub fn new(code: u8, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(NOT_FOUND, msg)
    }

    pub fn not_implemented(cmd: &str) -> Self {
        Self::new(NOT_IMPLEMENTED, format!("`{cmd}` is not implemented yet"))
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for CliError {}

/// Exit code for any error: typed errors keep their code, a missing account
/// is "not found", everything else is a plain failure.
pub fn code_for(err: &anyhow::Error) -> u8 {
    if let Some(e) = err.downcast_ref::<CliError>() {
        return e.code;
    }
    if let Some(so_accounts::AccountsError::NotFound(_)) =
        err.downcast_ref::<so_accounts::AccountsError>()
    {
        return NOT_FOUND;
    }
    FAILURE
}
