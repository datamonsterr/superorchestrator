//! Secret redaction shared by the supervisor log (SPEC 20) and the web API.

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
    if !at_boundary {
        return None;
    }
    if let Some(bearer) = match_bearer(rest) {
        return Some(bearer);
    }
    PREFIXES.iter().find_map(|p| {
        let after = rest.strip_prefix(p)?;
        let n = token_len(after);
        (n >= MIN_TOKEN).then(|| (MARK.to_string(), p.len() + n))
    })
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
    fn redacts_json_strings_deeply() {
        let mut v = serde_json::json!({"a": ["x sk-abcdef1234567890"], "b": {"c": "ok"}, "n": 1});
        redact_json(&mut v);
        assert_eq!(v["a"][0], "x [REDACTED]");
        assert_eq!(v["b"]["c"], "ok");
    }
}
