//! TypeScript types for the frontend, generated from the Rust types through their JSON
//! Schema. `cargo run -p so-web --bin gen-ts` writes `web/src/api/generated.ts`; a test
//! fails when the committed file is stale.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::Value;

use crate::types::*;

/// Where the generated file lives, relative to the repository root.
pub const OUTPUT: &str = "web/src/api/generated.ts";

const PRELUDE: &str = "\
export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}
";

fn roots(g: &mut SchemaGenerator) {
    fn add<T: JsonSchema>(g: &mut SchemaGenerator) {
        g.subschema_for::<T>();
    }
    add::<so_core::Event>(g);
    add::<so_core::Headroom>(g);
    add::<so_core::Choice>(g);
    add::<so_core::proto::HarnessesView>(g);
    add::<so_core::proto::AccountView>(g);
    add::<so_core::proto::RouteResult>(g);
    add::<so_core::proto::RouteParams>(g);
    add::<so_core::proto::MessageView>(g);
    add::<so_core::proto::SendMessageParams>(g);
    add::<so_core::proto::AckResult>(g);
    add::<so_core::proto::StatusResult>(g);
    add::<so_core::proto::AgentsView>(g);
    add::<so_supervisor::doctor::DoctorReport>(g);
    add::<RunViewPublic>(g);
    add::<JoinView>(g);
    add::<StatusView>(g);
    add::<SessionSummary>(g);
    add::<SessionDetail>(g);
    add::<DailyUsage>(g);
    add::<DashboardView>(g);
    add::<TranscriptPage>(g);
    add::<RouteRecord>(g);
    add::<WorktreeView>(g);
    add::<PruneRequest>(g);
    add::<PruneResult>(g);
    add::<ChoicesView>(g);
    add::<ConfigView>(g);
    add::<AddAccountRequest>(g);
    add::<ImportAccountRequest>(g);
    add::<ImportCandidate>(g);
    add::<PatchAccountRequest>(g);
    add::<FlagRequest>(g);
    add::<SignInStart>(g);
    add::<SignInStatus>(g);
    add::<TokenRequest>(g);
    add::<ApiErrorBody>(g);
    add::<StreamHello>(g);
    add::<SendMessageBody>(g);
    add::<RejectBody>(g);
    add::<ForkBody>(g);
    add::<so_core::proto::SpawnResult>(g);
    add::<ModelCatalog>(g);
    add::<AgentDetail>(g);
    add::<CreateAgentRequest>(g);
    add::<UpdateAgentRequest>(g);
    add::<AgentWriteResult>(g);
    add::<WorkflowSummary>(g);
    add::<RunWorkflowBody>(g);
    add::<so_core::proto::RunWorkflowResult>(g);
    add::<so_core::proto::WorkflowRunView>(g);
    add::<so_core::proto::WorkflowStatusResult>(g);
    add::<so_core::proto::WorkflowEventData>(g);
}

fn literal(v: &Value) -> String {
    v.to_string()
}

fn ts_type(s: &Value) -> String {
    let Some(o) = s.as_object() else {
        return if s == &Value::Bool(false) {
            "never"
        } else {
            "unknown"
        }
        .to_string();
    };
    if let Some(r) = o.get("$ref").and_then(Value::as_str) {
        return r.rsplit('/').next().unwrap_or("unknown").to_string();
    }
    if let Some(c) = o.get("const") {
        return literal(c);
    }
    if let Some(e) = o.get("enum").and_then(Value::as_array) {
        return e.iter().map(literal).collect::<Vec<_>>().join(" | ");
    }
    for (key, sep) in [("oneOf", " | "), ("anyOf", " | "), ("allOf", " & ")] {
        if let Some(a) = o.get(key).and_then(Value::as_array) {
            return a.iter().map(ts_type).collect::<Vec<_>>().join(sep);
        }
    }
    match o.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .map(|t| {
                let mut one = o.clone();
                one.insert("type".into(), Value::String(t.into()));
                ts_type(&Value::Object(one))
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Some(Value::String(t)) => match t.as_str() {
            "string" => "string".into(),
            "integer" | "number" => "number".into(),
            "boolean" => "boolean".into(),
            "null" => "null".into(),
            "array" => array_type(o),
            "object" => object_type(o),
            _ => "unknown".into(),
        },
        _ => "unknown".into(),
    }
}

fn array_type(o: &serde_json::Map<String, Value>) -> String {
    if let Some(p) = o.get("prefixItems").and_then(Value::as_array) {
        let items: Vec<String> = p.iter().map(ts_type).collect();
        return format!("[{}]", items.join(", "));
    }
    match o.get("items") {
        Some(i) => {
            let t = ts_type(i);
            if t.contains(' ') {
                format!("({t})[]")
            } else {
                format!("{t}[]")
            }
        }
        None => "unknown[]".into(),
    }
}

fn object_type(o: &serde_json::Map<String, Value>) -> String {
    let props = o.get("properties").and_then(Value::as_object);
    let extra = o
        .get("additionalProperties")
        .filter(|a| a.is_object() || **a == Value::Bool(true));
    match (props, extra) {
        (None, Some(a)) => format!("Record<string, {}>", ts_type(a)),
        (None, None) => "Record<string, unknown>".into(),
        (Some(p), _) if p.is_empty() => "Record<string, never>".into(),
        (Some(p), _) => {
            let required: Vec<&str> = o
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let fields: Vec<String> = sorted(p)
                .into_iter()
                .map(|(k, v)| {
                    let opt = if required.contains(&k.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    format!("{k}{opt}: {}", ts_type(v))
                })
                .collect();
            format!("{{ {} }}", fields.join("; "))
        }
    }
}

/// Properties by name, so the output does not depend on serde_json's `preserve_order`
/// feature (which other crates in the workspace switch on).
fn sorted(p: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut v: Vec<_> = p.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

fn doc(s: &Value) -> String {
    s.get("description")
        .and_then(Value::as_str)
        .map(|d| {
            let one = d
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace("*/", "* /");
            format!("/** {one} */\n")
        })
        .unwrap_or_default()
}

fn definition(name: &str, s: &Value) -> String {
    let is_plain_object = s.get("type").and_then(Value::as_str) == Some("object")
        && s.get("properties").is_some()
        && s.get("oneOf").is_none()
        && s.get("allOf").is_none();
    if !is_plain_object {
        return format!("{}export type {name} = {};\n", doc(s), ts_type(s));
    }
    let required: Vec<&str> = s
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = format!("{}export interface {name} {{\n", doc(s));
    if let Some(p) = s.get("properties").and_then(Value::as_object) {
        for (k, v) in sorted(p) {
            let opt = if required.contains(&k.as_str()) {
                ""
            } else {
                "?"
            };
            out.push_str(&format!(
                "  {}{k}{opt}: {};\n",
                doc(v).trim_end().replace('\n', " ") + if doc(v).is_empty() { "" } else { "\n  " },
                ts_type(v)
            ));
        }
    }
    out.push_str("}\n");
    out
}

/// The whole generated file.
pub fn render() -> String {
    let mut g = SchemaSettings::draft2020_12().into_generator();
    roots(&mut g);
    let defs = g.definitions().clone();
    let mut names: Vec<&String> = defs.keys().collect();
    names.sort();
    let mut out = String::from(
        "// Generated by `cargo run -p so-web --bin gen-ts`. Do not edit by hand.\n\n",
    );
    out.push_str(PRELUDE);
    for n in names {
        out.push('\n');
        out.push_str(&definition(n, &defs[n]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(OUTPUT);
        std::fs::read_to_string(&path).unwrap_or_default()
    }

    #[test]
    fn committed_typescript_is_up_to_date() {
        assert!(
            committed() == render(),
            "{OUTPUT} is stale: run `cargo run -p so-web --bin gen-ts` and commit the result"
        );
    }

    #[test]
    fn output_has_the_key_types_and_no_join_fields_on_the_public_run() {
        let ts = render();
        for want in [
            "export interface RunViewPublic",
            "export interface JoinView",
            "export type SignInStart",
            "export interface Event",
            "export type RunState",
        ] {
            assert!(ts.contains(want), "missing {want}");
        }
        let run = ts.split("export interface RunViewPublic").nth(1).unwrap();
        let run = run.split("\n}\n").next().unwrap();
        for leak in ["join_command", "join_cwd", "join_env"] {
            assert!(!run.contains(leak), "{leak} leaked into RunViewPublic");
        }
        assert!(ts.contains("session_id: string"));
    }
}
