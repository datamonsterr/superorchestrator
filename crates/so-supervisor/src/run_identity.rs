//! Hashing shared by run identity issuance and hello verification.

use sha2::{Digest, Sha256};

pub(crate) fn run_token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}
