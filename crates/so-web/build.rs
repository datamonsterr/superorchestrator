//! Copies `web/dist` into `$OUT_DIR/dist` for embedding, or writes a placeholder page when
//! the frontend has not been built, so `cargo build` works without pnpm.

use std::path::Path;
use std::{env, fs, io};

const PLACEHOLDER: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Superorchestrator</title></head><body><h1>Superorchestrator</h1>\
<p>The web UI is not built. Run <code>pnpm install &amp;&amp; pnpm build</code> in \
<code>web/</code> and rebuild.</p></body></html>";

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dist = Path::new(&manifest).join("../../web/dist");
    let out = Path::new(&env::var("OUT_DIR").unwrap_or_default()).join("dist");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", dist.display());
    let _ = fs::remove_dir_all(&out);
    let built = dist.join("index.html").is_file() && copy_dir(&dist, &out).is_ok();
    if !built {
        let _ = fs::remove_dir_all(&out);
        if let Err(e) =
            fs::create_dir_all(&out).and_then(|()| fs::write(out.join("index.html"), PLACEHOLDER))
        {
            println!("cargo:warning=could not write the placeholder UI: {e}");
        }
    }
}
