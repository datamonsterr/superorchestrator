//! Turning a workflow source file into one async function body.

/// JS installed before the script: the public API on top of the native hooks.
pub(crate) const PRELUDE: &str = include_str!("prelude.js");

/// Wraps `source` in an async IIFE.
///
/// Two script styles are accepted:
/// - body style (Claude Code): top-level `await` and `return`, optionally starting with
///   `export const meta = {...}`;
/// - `export default async function (args) { ... }`, called with `args` after the body ran.
///
/// Only `export` keywords at the start of a line are rewritten. The wrapper waits for all
/// fire-and-forget `log()` calls to reach the host before the run finishes.
pub(crate) fn wrap(source: &str) -> String {
    let mut body = String::with_capacity(source.len() + 128);
    for line in source.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        if let Some(rest) = trimmed.strip_prefix("export default ") {
            body.push_str(indent);
            body.push_str("const __default = ");
            body.push_str(rest);
        } else if let Some(rest) = trimmed.strip_prefix("export ") {
            body.push_str(indent);
            body.push_str(rest);
        } else {
            body.push_str(line);
        }
        body.push('\n');
    }
    format!(
        "(async () => {{ try {{\n{body}\n;if (typeof __default === 'function') \
         return await __default(args);\n}} finally {{ await __drain(); }} }})()"
    )
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn rewrites_exports() {
        let w = wrap("export const meta = {name:'x'}\n  export default async function(a){}\n");
        assert!(w.contains("\nconst meta = {name:'x'}\n"));
        assert!(w.contains("  const __default = async function(a){}"));
        assert!(!w.contains("export"));
    }
}
