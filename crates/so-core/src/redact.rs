//! Secret redaction shared by the supervisor log (SPEC 20) and the web API.

const MARK: &str = "[REDACTED]";
/// Prefixes of API keys and tokens. Longer prefixes first.
const PREFIXES: [&str; 8] = [
    "github_pat_",
    "sk-or-",
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
];
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
    if !at_boundary {
        return None;
    }
    if let Some(bearer) = match_bearer(rest) {
        return Some(bearer);
    }
    if let Some(n) = match_structured(rest) {
        return Some((MARK.to_string(), n));
    }
    PREFIXES.iter().find_map(|p| {
        let after = rest.strip_prefix(p)?;
        let n = token_len(after);
        (n >= MIN_TOKEN).then(|| (MARK.to_string(), p.len() + n))
    })
}

/// Slack tokens (`xox[bpars]-...`), AWS access key ids (`AKIA` + 16 characters) and JWTs
/// (`eyJ....eyJ....signature`). Returns the number of bytes matched.
fn match_structured(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if rest.starts_with("xox") && b.len() > 4 && b"bpars".contains(&b[3]) && b[4] == b'-' {
        let n = token_len(&rest[5..]);
        return (n >= MIN_TOKEN).then_some(5 + n);
    }
    if let Some(after) = rest.strip_prefix("AKIA") {
        let n = after
            .bytes()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .count();
        return (n >= 16).then_some(4 + 16);
    }
    if rest.starts_with("eyJ") {
        let seg = |s: &str| {
            s.bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                .count()
        };
        let first = seg(rest);
        let r = rest[first..].strip_prefix('.')?;
        if !r.starts_with("eyJ") {
            return None;
        }
        let second = seg(r);
        let sig = rest[first + 1 + second..].strip_prefix('.')?;
        return Some(first + 1 + second + 1 + seg(sig));
    }
    None
}

fn token_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| is_token_char(*c))
        .map(char::len_utf8)
        .sum()
}

/// `Bearer <token>`: the word is case-insensitive and any whitespace may follow.
fn match_bearer(rest: &str) -> Option<(String, usize)> {
    const WORD: &str = "bearer";
    let head = rest.get(..WORD.len())?;
    if !head.eq_ignore_ascii_case(WORD) {
        return None;
    }
    let after_word = &rest[WORD.len()..];
    let ws: usize = after_word
        .chars()
        .take_while(|c| c.is_whitespace())
        .map(char::len_utf8)
        .sum();
    if ws == 0 {
        return None;
    }
    let n = token_len(&after_word[ws..]);
    if n == 0 {
        return None;
    }
    let keep = &rest[..WORD.len() + ws];
    Some((format!("{keep}{MARK}"), keep.len() + n))
}

/// Redacts every string inside a JSON value, in place.
pub fn redact_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            let r = redact(s);
            if r != *s {
                *s = r;
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(redact_json),
        serde_json::Value::Object(o) => o.values_mut().for_each(redact_json),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            (
                "authorization: bearer abc123",
                "authorization: bearer [REDACTED]",
            ),
            ("BEARER\tabc123 x", "BEARER\t[REDACTED] x"),
            ("Bearer   abc123", "Bearer   [REDACTED]"),
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

    #[test]
    fn redacts_more_secret_shapes() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r";
        let cases = [
            (
                "t github_pat_11ABCDEFG0abcdefghij_klmnop x",
                "t [REDACTED] x",
            ),
            ("t xoxb-123456789012-abcdefABCDEF x", "t [REDACTED] x"),
            ("id AKIAIOSFODNN7EXAMPLE end", "id [REDACTED] end"),
            (&format!("jwt {jwt} end"), "jwt [REDACTED] end"),
            ("ghs_ABCDEFGH12345678", "[REDACTED]"),
        ];
        for (input, want) in cases {
            assert_eq!(redact(input), want, "{input}");
        }
        for ok in ["AKIA short", "eyJnotajwt", "xoxz-123456789", "my xox-thing"] {
            assert_eq!(redact(ok), ok);
        }
    }

    #[test]
    fn redacts_json_strings_deeply() {
        let mut v = serde_json::json!({"a": ["x sk-abcdef1234567890"], "b": {"c": "ok"}, "n": 1});
        redact_json(&mut v);
        assert_eq!(v["a"][0], "x [REDACTED]");
        assert_eq!(v["b"]["c"], "ok");
    }
}
