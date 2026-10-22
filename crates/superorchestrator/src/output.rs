//! One place for human vs JSON output.

use std::io::{self, Write};

use anyhow::Result;
use serde::Serialize;

use crate::ctx::Ctx;

impl Ctx {
    /// With `--json`, prints `value` as pretty JSON; otherwise runs `human`.
    pub fn emit<T: Serialize>(
        &self,
        value: &T,
        human: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<()> {
        let mut out = io::stdout().lock();
        if self.json {
            serde_json::to_writer_pretty(&mut out, value)?;
            writeln!(out)?;
        } else {
            human(&mut out)?;
        }
        Ok(())
    }

    /// Action result: `{ "ok": true, ...extra }` or the human line.
    pub fn emit_ok(&self, extra: serde_json::Value, line: &str) -> Result<()> {
        let mut obj = serde_json::json!({ "ok": true });
        if let (Some(o), serde_json::Value::Object(e)) = (obj.as_object_mut(), extra) {
            o.extend(e);
        }
        self.emit(&obj, |w| writeln!(w, "{line}"))
    }
}

/// Writes a left-aligned table.
pub fn write_table(w: &mut dyn Write, header: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut line = |cells: Vec<&str>| -> io::Result<()> {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        writeln!(w, "{}", parts.join("  ").trim_end())
    };
    line(header.to_vec())?;
    for r in rows {
        line(r.iter().map(String::as_str).collect())?;
    }
    Ok(())
}
