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
/// Google API keys: `AIza` plus 35 characters.
const GOOGLE_PREFIX: &str = "AIza";
const GOOGLE_MIN: usize = 35;
/// Hugging Face tokens: `hf_` plus at least 30 alphanumeric characters.
const HF_PREFIX: &str = "hf_";
const HF_MIN: usize = 30;
/// Longest JWT segment scanned, so a long run of `eyJ-` cannot make redaction quadratic.
const MAX_JWT_SEGMENT: usize = 4096;

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '+' | '/')
}

/// Replaces `sk-`, `sk-or-`, `ghp_`, `gho_`, `AIza` and `hf_` tokens and
/// `Bearer <x>` values.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    let mut jwt_dead_until = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        let at_boundary = prev.is_none_or(|p| !p.is_ascii_alphanumeric());
        if let Some(skip) = match_secret(rest, at_boundary, i, &mut jwt_dead_until) {
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
fn match_secret(
    rest: &str,
    at_boundary: bool,
    abs: usize,
    jwt_dead_until: &mut usize,
) -> Option<(String, usize)> {
    if !at_boundary {
        return None;
    }
    if let Some(bearer) = match_bearer(rest) {
        return Some(bearer);
    }
    if let Some(n) = match_structured(rest, abs, jwt_dead_until) {
        return Some((MARK.to_string(), n));
    }
    PREFIXES.iter().find_map(|p| {
        let after = rest.strip_prefix(p)?;
        let n = token_len(after);
        (n >= MIN_TOKEN).then(|| (MARK.to_string(), p.len() + n))
    })
}

/// Slack tokens (`xox[bpars]-...`), AWS access key ids (`AKIA` + 16 characters),
/// Google API keys (`AIza` + 35 characters), Hugging Face tokens (`hf_` + 30
/// alphanumerics) and JWTs (`eyJ....eyJ....signature`). Returns the number of
/// bytes matched.
fn match_structured(rest: &str, abs: usize, jwt_dead_until: &mut usize) -> Option<usize> {
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
        return (n >= 16).then_some(4 + n);
    }
    if let Some(after) = rest.strip_prefix(GOOGLE_PREFIX) {
        let n = after
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            .count();
        return (n >= GOOGLE_MIN).then_some(GOOGLE_PREFIX.len() + n);
    }
    if let Some(after) = rest.strip_prefix(HF_PREFIX) {
        let n = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
        return (n >= HF_MIN).then_some(HF_PREFIX.len() + n);
    }
    // After a failed attempt every start inside the same token run fails the same way.
    if rest.starts_with("eyJ") && abs >= *jwt_dead_until {
        let seg = |s: &str| {
            s.bytes()
                .take(MAX_JWT_SEGMENT)
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                .count()
        };
        let first = seg(rest);
        *jwt_dead_until = abs + first;
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
    fn akia_redacts_the_whole_run_and_jwt_scan_is_bounded() {
        let long = format!("id AKIA{} end", "A1".repeat(12));
        assert_eq!(redact(&long), "id [REDACTED] end");
        let adversarial = "eyJ-".repeat(16 * 1024);
        let start = std::time::Instant::now();
        let out = redact(&adversarial);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(out.len(), adversarial.len(), "not a JWT, left alone");
    }

    #[test]
    fn redacts_google_api_keys_and_hugging_face_tokens() {
        let google = format!("AIza{}", "Ab1_-".repeat(7));
        let hf = format!("hf_{}", "aB1".repeat(10));
        let cases = [
            (format!("key {google} end"), "key [REDACTED] end"),
            (format!("k={hf}&next"), "k=[REDACTED]&next"),
        ];
        for (input, want) in cases {
            assert_eq!(redact(&input), want, "{input}");
        }
        let short_google = format!("AIza{}", "A".repeat(34));
        let short_hf = format!("hf_{}", "a".repeat(29));
        let near_misses = [
            short_google,
            short_hf,
            "AIza".to_string(),
            "hf_x".to_string(),
        ];
        for s in near_misses {
            assert_eq!(redact(&s), s, "{s}");
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
