//! Pagination helpers: the `limit` clamp and `(ts, id)` cursors.

use chrono::{DateTime, SecondsFormat, Utc};

use crate::error::{ApiError, ApiResult};

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 1000;

/// `limit` defaults to 200 and is clamped to 1..=1000.
pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Encodes a `(ts, id)` position as an opaque token (hex of `ts|id`).
pub fn encode_cursor(ts: &DateTime<Utc>, id: &str) -> String {
    let raw = format!("{}|{id}", ts.to_rfc3339_opts(SecondsFormat::Micros, true));
    raw.bytes().map(|b| format!("{b:02x}")).collect()
}

pub fn decode_cursor(cursor: &str) -> ApiResult<(DateTime<Utc>, String)> {
    let bad = || ApiError::invalid("invalid cursor");
    if !cursor.len().is_multiple_of(2) || !cursor.is_ascii() {
        return Err(bad());
    }
    let bytes: Vec<u8> = (0..cursor.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cursor[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    let raw = String::from_utf8(bytes).map_err(|_| bad())?;
    let (ts, id) = raw.split_once('|').ok_or_else(bad)?;
    let ts = DateTime::parse_from_rfc3339(ts)
        .map_err(|_| bad())?
        .with_timezone(&Utc);
    Ok((ts, id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), 200);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(50_000)), 1000);
    }

    #[test]
    fn cursor_round_trips_and_rejects_garbage() {
        let ts = Utc::now();
        let c = encode_cursor(&ts, "abc-1");
        let (t2, id) = decode_cursor(&c).unwrap();
        assert_eq!(id, "abc-1");
        assert_eq!(t2.timestamp_micros(), ts.timestamp_micros());
        for bad in ["", "zz", "abc", "6162", "é"] {
            assert!(decode_cursor(bad).is_err(), "{bad}");
        }
    }
}
