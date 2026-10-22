//! Conversions between Rust values and SQLite text columns.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Row;
use rusqlite::types::Type;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// RFC3339 UTC with fixed microsecond width, so text order equals time order.
pub fn ts_to_sql(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn opt_ts_to_sql(t: &Option<DateTime<Utc>>) -> Option<String> {
    t.as_ref().map(ts_to_sql)
}

/// Encodes a unit-variant serde enum as its plain string.
pub fn enum_to_sql<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        // Unit enums always serialise to strings; fall back to JSON text otherwise.
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn conv_err(idx: usize, e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e))
}

pub fn get_enum<T: DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_value(serde_json::Value::String(s)).map_err(|e| conv_err(idx, e))
}

pub fn get_opt_enum<T: DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| serde_json::from_value(serde_json::Value::String(s)).map_err(|e| conv_err(idx, e)))
        .transpose()
}

fn parse_ts(idx: usize, s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| conv_err(idx, e))
}

pub fn get_ts(row: &Row<'_>, idx: usize) -> rusqlite::Result<DateTime<Utc>> {
    let s: String = row.get(idx)?;
    parse_ts(idx, &s)
}

pub fn get_opt_ts(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| parse_ts(idx, &s)).transpose()
}

/// Current time truncated to the stored microsecond precision (for round-trip tests).
#[cfg(test)]
pub fn now() -> DateTime<Utc> {
    use chrono::SubsecRound;
    Utc::now().trunc_subsecs(6)
}
