//! Token redaction for the supervisor log (SPEC 20).

use std::io::{self, Write};

const MARK: &str = "[REDACTED]";
/// Prefixes of API keys and tokens. Longer prefixes first.
const PREFIXES: [&str; 4] = ["sk-or-", "sk-", "ghp_", "gho_"];
const MIN_TOKEN: usize = 8;

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '+' | '/')
}

/// Replaces `sk-`, `sk-or-`, `ghp_`, `gho_` tokens and `Bearer <x>` values.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < text.len() {
        let rest = &text[i..];
        let at_boundary = prev.is_none_or(|p| !p.is_ascii_alphanumeric());
        if let Some(skip) = match_secret(rest, at_boundary) {
            out.push_str(&skip.0);
            i += skip.1;
            prev = Some(']');
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        out.push(c);
        i += c.len_utf8();
        prev = Some(c);
    }
    out
}

/// Returns the replacement text and the number of bytes consumed.
fn match_secret(rest: &str, at_boundary: bool) -> Option<(String, usize)> {
    if let Some(after) = rest.strip_prefix("Bearer ") {
        let n: usize = after
            .chars()
            .take_while(|c| is_token_char(*c))
            .map(char::len_utf8)
            .sum();
        if n > 0 && at_boundary {
            return Some((format!("Bearer {MARK}"), "Bearer ".len() + n));
        }
        return None;
    }
    if !at_boundary {
        return None;
    }
    PREFIXES.iter().find_map(|p| {
        let after = rest.strip_prefix(p)?;
        let n: usize = after
            .chars()
            .take_while(|c| is_token_char(*c))
            .map(char::len_utf8)
            .sum();
        (n >= MIN_TOKEN).then(|| (MARK.to_string(), p.len() + n))
    })
}

/// Wraps a writer and redacts each complete line before it is written.
pub struct RedactWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> RedactWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.inner.write_all(redact(&text).as_bytes())
    }
}

impl<W: Write> Write for RedactWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.emit(&line)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactWriter<W> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let _ = self.emit(&rest);
        }
        let _ = self.inner.flush();
    }
}

/// `MakeWriter` that hands out a redacting writer per event.
pub struct MakeRedacting<F>(pub F);

impl<'a, W, F> tracing_subscriber::fmt::MakeWriter<'a> for MakeRedacting<F>
where
    W: Write,
    F: Fn() -> W + 'a,
{
    type Writer = RedactWriter<W>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactWriter::new((self.0)())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn redacts_known_patterns() {
        let cases = [
            ("key sk-abcdef1234567890 end", "key [REDACTED] end"),
            ("k=sk-or-v1-abcdef123456", "k=[REDACTED]"),
            ("token ghp_ABCDEFGH12345678 x", "token [REDACTED] x"),
            ("gho_ABCDEFGH12345678", "[REDACTED]"),
            (
                "Authorization: Bearer abc.def-123_xyz=",
                "Authorization: Bearer [REDACTED]",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact(input), want, "{input}");
        }
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for s in [
            "task-force-members running",
            "sk-short",
            "Bearer",
            "no secrets here: ünïcode ✓",
        ] {
            assert_eq!(redact(s), s);
        }
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, d: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(d);
            Ok(d.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_redacts_across_split_writes_and_on_drop() {
        let sink = Sink::default();
        {
            let mut w = RedactWriter::new(sink.clone());
            w.write_all(b"auth sk-abcdef").unwrap();
            w.write_all(b"1234567890\nsecond ghp_ABCDEFGH12345678")
                .unwrap();
        }
        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(out, "auth [REDACTED]\nsecond [REDACTED]");
    }
}
