//! Writes `web/src/api/generated.ts` (or with `--check` fails when it is stale).

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(so_web::tsgen::OUTPUT);
    let text = so_web::tsgen::render();
    if std::env::args().any(|a| a == "--check") {
        return if std::fs::read_to_string(&path).is_ok_and(|c| c == text) {
            ExitCode::SUCCESS
        } else {
            eprintln!(
                "{} is stale; run `cargo run -p so-web --bin gen-ts`",
                path.display()
            );
            ExitCode::FAILURE
        };
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
