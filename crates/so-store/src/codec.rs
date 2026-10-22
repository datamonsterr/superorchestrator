//! Conversions between Rust values and SQLite text columns.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Row;
use rusqlite::types::Type;
use std::str::FromStr;

/// RFC3339 UTC with fixed microsecond width, so text order equals time order.
pub fn ts_to_sql(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn opt_ts_to_sql(t: &Option<DateTime<Utc>>) -> Option<String> {
    t.as_ref().map(ts_to_sql)
}

/// Encodes an enum as its stable wire name (`Display`, i.e. `as_str`).
pub fn enum_to_sql<T: std::fmt::Display>(v: &T) -> String {
    v.to_string()
}

fn conv_err(idx: usize, e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e))
}

pub fn get_enum<T: FromStr>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e| conv_err(idx, e))
}

pub fn get_opt_enum<T: FromStr>(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: Option<String> = row.get(idx)?;
    s.map(|s| s.parse().map_err(|e| conv_err(idx, e)))
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
