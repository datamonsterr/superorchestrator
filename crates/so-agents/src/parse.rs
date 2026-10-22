//! `AGENT.md` parsing (SPEC 10): YAML frontmatter plus a Markdown body.
//!
//! The frontmatter is a superset of Claude Code's subagent format
//! (<https://code.claude.com/docs/en/sub-agents>): `tools` and
//! `disallowedTools` accept a comma-separated string or a list, `mcpServers`
//! mixes server names and inline definitions, and unknown fields (`color`,
//! `permissionMode`, ...) are ignored as Claude Code does. Superorchestrator
//! adds `harness`, `accounts` and `review`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use so_core::{Harness, Isolation};

use crate::error::{AgentError, Result};

/// The `harness` field: let the router pick, or pin one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessPref {
    Auto,
    #[serde(untagged)]
    Fixed(Harness),
}

impl HarnessPref {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        s.to_ascii_lowercase()
            .parse::<Harness>()
            .map(Self::Fixed)
            .map_err(|_| format!("unknown harness `{s}` (auto, codex, agy, opencode, claude)"))
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(h) => h.as_str(),
        }
    }
}

/// Where an agent file was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// `<cwd>/.superorchestrator/agents/`
    Superorchestrator,
    /// `<config dir>/agents/`
    User,
    /// `<cwd>/.claude/agents/`, read-only.
    ClaudeImport,
}

impl AgentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Superorchestrator => "superorchestrator",
            Self::User => "user",
            Self::ClaudeImport => "claude",
        }
    }

    pub fn read_only(&self) -> bool {
        matches!(self, Self::ClaudeImport)
    }
}

/// `memory` scope. Files: `user` = `<config>/memory/MEMORY.md`,
/// `project` = `<cwd>/.superorchestrator/MEMORY.md`,
/// `local` = `<cwd>/.superorchestrator/MEMORY.local.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Memory {
    User,
    Project,
    Local,
}

/// An `mcpServers` entry: a server name, or an inline definition.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum McpRef {
    Name(String),
    Inline { name: String, config: Value },
}

impl McpRef {
    pub fn name(&self) -> &str {
        match self {
            Self::Name(n) | Self::Inline { name: n, .. } => n,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    pub harness: HarnessPref,
    pub accounts: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<McpRef>,
    pub hooks: Value,
    pub memory: Option<Memory>,
    pub max_turns: Option<u32>,
    pub isolation: Option<Isolation>,
    pub review: bool,
    /// Markdown body: the system prompt.
    pub prompt: String,
    /// The folder holding `AGENT.md`, `skills/` and extra files. `None` for a flat `<name>.md`.
    pub dir: Option<PathBuf>,
    /// The file that was parsed.
    pub file: PathBuf,
    pub source: AgentSource,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum StringList {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl StringList {
    /// Splits a comma-separated string at top-level commas (not inside parentheses,
    /// so `Bash(git add:*, git commit:*)` stays one entry).
    fn into_vec(self) -> Vec<String> {
        let items = match self {
            Self::None => return Vec::new(),
            Self::Many(v) => v,
            Self::One(s) => split_top_level(&s),
        };
        items
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

fn split_top_level(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth <= 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    name: Option<String>,
    description: Option<String>,
    harness: Option<String>,
    #[serde(default)]
    accounts: StringList,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    tools: StringList,
    #[serde(default)]
    disallowed_tools: StringList,
    #[serde(default)]
    skills: StringList,
    #[serde(default)]
    mcp_servers: Vec<Value>,
    #[serde(default)]
    hooks: Option<Value>,
    memory: Option<Memory>,
    max_turns: Option<u32>,
    isolation: Option<String>,
    #[serde(default)]
    review: bool,
}

/// Valid agent names: no `:` or `/`, not starting with `-` (Claude Code's rule), plain characters.
pub fn validate_agent_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('-')
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(AgentError::InvalidName(name.to_string()))
    }
}

/// Largest agent file accepted.
pub const MAX_AGENT_FILE: u64 = 512 * 1024;
/// Largest memory file read; the rest is cut off with a note.
pub const MAX_MEMORY_FILE: u64 = 256 * 1024;

/// Reads at most `max` bytes. Returns the (lossy UTF-8) text and whether it was cut.
pub(crate) fn read_capped(path: &Path, max: u64) -> std::io::Result<(String, bool)> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_end(&mut buf)?;
    let cut = buf.len() as u64 > max;
    if cut {
        buf.truncate(max as usize);
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), cut))
}

/// True when the YAML has an anchor (`&x`) or alias (`*x`) node. serde_norway
/// replays aliases without bounding expansion, so they are rejected outright
/// (a billion-laughs file would otherwise blow up memory). Quoted text and
/// comments are skipped; block scalars mentioning `&word` are rejected too.
fn has_anchor_or_alias(yaml: &str) -> bool {
    for line in yaml.lines() {
        let (mut quote, mut prev) = (None::<char>, ' ');
        let chars: Vec<char> = line.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            match quote {
                Some(q) => {
                    if c == q && prev != '\\' {
                        quote = None;
                    }
                }
                None => match c {
                    '"' | '\'' if prev.is_whitespace() || "[{,:-".contains(prev) => quote = Some(c),
                    '#' if prev.is_whitespace() => break,
                    '&' | '*'
                        if (prev.is_whitespace() || "[{,".contains(prev))
                            && chars
                                .get(i + 1)
                                .is_some_and(|n| n.is_alphanumeric() || *n == '_') =>
                    {
                        return true;
                    }
                    _ => {}
                },
            }
            prev = c;
        }
    }
    false
}

/// Splits `---` fenced frontmatter from the body.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

fn perr(file: &Path, msg: impl Into<String>) -> AgentError {
    AgentError::Parse {
        path: file.to_path_buf(),
        msg: msg.into(),
    }
}

/// The MCP server every run gets for reaching the supervisor; an agent may not define or
/// replace it.
pub const RESERVED_MCP_NAME: &str = "superorchestrator";

fn mcp_ref(file: &Path, v: Value) -> Result<McpRef> {
    let reserved = |name: &str| {
        if name == RESERVED_MCP_NAME {
            return Err(perr(
                file,
                format!("`{RESERVED_MCP_NAME}` is reserved: every run already has that MCP server"),
            ));
        }
        Ok(())
    };
    match v {
        Value::String(s) => {
            reserved(&s)?;
            Ok(McpRef::Name(s))
        }
        Value::Object(m) if m.len() == 1 => {
            let (name, config) = m.into_iter().next().unwrap_or_default();
            reserved(&name)?;
            Ok(McpRef::Inline { name, config })
        }
        _ => Err(perr(
            file,
            "mcpServers entries are a name or a single `name: {definition}` map",
        )),
    }
}

/// Parses one agent file. `fallback_name` (the folder or file stem) is used when
/// the frontmatter has no `name`.
pub fn parse_agent(
    text: &str,
    file: &Path,
    dir: Option<PathBuf>,
    source: AgentSource,
    fallback_name: &str,
) -> Result<AgentDef> {
    if text.len() as u64 > MAX_AGENT_FILE {
        return Err(perr(
            file,
            format!("larger than {} KiB", MAX_AGENT_FILE / 1024),
        ));
    }
    let (fm, body) =
        split_frontmatter(text).ok_or_else(|| perr(file, "missing `---` YAML frontmatter"))?;
    if has_anchor_or_alias(fm) {
        return Err(perr(file, "YAML anchors and aliases are not supported"));
    }
    let raw: Raw = serde_norway::from_str(fm).map_err(|e| perr(file, e.to_string()))?;
    let name = raw.name.unwrap_or_else(|| fallback_name.to_string());
    validate_agent_name(&name)?;
    let skills = raw.skills.into_vec();
    for sk in &skills {
        validate_agent_name(sk)?;
    }
    // A tool entry that starts with `-` would become a command-line flag of the harness.
    let tools = raw.tools.into_vec();
    let disallowed_tools = raw.disallowed_tools.into_vec();
    for (field, list) in [("tools", &tools), ("disallowedTools", &disallowed_tools)] {
        if let Some(bad) = list.iter().find(|t| t.trim_start().starts_with('-')) {
            return Err(perr(
                file,
                format!("`{field}` entry `{bad}` starts with `-`: tool names are not flags"),
            ));
        }
    }
    let description = raw
        .description
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| perr(file, "`description` is required"))?;
    let harness = match raw.harness {
        Some(h) => HarnessPref::parse(&h).map_err(|m| perr(file, m))?,
        None => HarnessPref::Auto,
    };
    let isolation = match raw.isolation.as_deref() {
        None => None,
        Some(s) => Some(
            s.parse::<Isolation>()
                .map_err(|_| perr(file, format!("unknown isolation `{s}` (worktree or none)")))?,
        ),
    };
    let mcp_servers = raw
        .mcp_servers
        .into_iter()
        .map(|v| mcp_ref(file, v))
        .collect::<Result<Vec<_>>>()?;
    // Claude Code's `inherit` means "no override".
    let model = raw.model.filter(|m| m != "inherit");
    Ok(AgentDef {
        name,
        description: description.trim().to_string(),
        harness,
        accounts: raw.accounts.into_vec(),
        model,
        effort: raw.effort,
        tools,
        disallowed_tools,
        skills,
        mcp_servers,
        hooks: raw
            .hooks
            .unwrap_or_else(|| Value::Object(Default::default())),
        memory: raw.memory,
        max_turns: raw.max_turns,
        isolation,
        review: raw.review,
        prompt: body.trim().to_string(),
        dir,
        file: file.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = "---
name: pr-reviewer
description: Reviews a PR diff for correctness bugs.
harness: auto            # or codex | agy | opencode | claude
accounts: [codex-acc2]   # optional allow-list
model: gpt-6.1-sol
effort: high
tools: [Read, Grep, Bash]
disallowedTools: [Write]
skills: [gh-cli]
mcpServers: [github]
hooks: {}
memory: project          # user | project | local
maxTurns: 40
isolation: worktree
review: false
---
System prompt body…
";

    fn parse(text: &str) -> Result<AgentDef> {
        parse_agent(
            text,
            Path::new("/x/AGENT.md"),
            None,
            AgentSource::User,
            "stem",
        )
    }

    #[test]
    fn spec_example() {
        let a = parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(a.name, "pr-reviewer");
        assert_eq!(a.harness, HarnessPref::Auto);
        assert_eq!(a.accounts, ["codex-acc2"]);
        assert_eq!(a.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(a.effort.as_deref(), Some("high"));
        assert_eq!(a.tools, ["Read", "Grep", "Bash"]);
        assert_eq!(a.disallowed_tools, ["Write"]);
        assert_eq!(a.skills, ["gh-cli"]);
        assert_eq!(a.mcp_servers, [McpRef::Name("github".into())]);
        assert_eq!(a.memory, Some(Memory::Project));
        assert_eq!(a.max_turns, Some(40));
        assert_eq!(a.isolation, Some(Isolation::Worktree));
        assert!(!a.review);
        assert_eq!(a.prompt, "System prompt body…");
        assert_eq!(a.hooks, serde_json::json!({}));
    }

    #[test]
    fn claude_code_spellings_and_comma_strings() {
        let a = parse(
            "---
name: browser-tester
description: Tests in a browser
tools: Read, Grep, Bash(git add:*, git commit:*)
disallowedTools: Write, Edit
model: inherit
permissionMode: default
color: cyan
mcpServers:
  - playwright:
      type: stdio
      command: npx
      args: [\"-y\", \"@playwright/mcp@latest\"]
  - github
harness: claude
---
body
",
        )
        .unwrap();
        assert_eq!(a.tools, ["Read", "Grep", "Bash(git add:*, git commit:*)"]);
        assert_eq!(a.disallowed_tools, ["Write", "Edit"]);
        assert_eq!(a.model, None);
        assert_eq!(a.harness, HarnessPref::Fixed(Harness::Claude));
        assert_eq!(a.mcp_servers.len(), 2);
        match &a.mcp_servers[0] {
            McpRef::Inline { name, config } => {
                assert_eq!(name, "playwright");
                assert_eq!(config["command"], "npx");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn repo_claude_agents_parse() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/agents");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let text = std::fs::read_to_string(&p).unwrap();
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            let a = parse_agent(&text, &p, None, AgentSource::ClaudeImport, &stem).unwrap();
            assert_eq!(a.name, stem);
            assert!(!a.prompt.is_empty());
            n += 1;
        }
        assert!(n >= 4);
    }

    #[test]
    fn errors() {
        assert!(parse("no frontmatter").is_err());
        assert!(
            parse("---\nname: a\n---\nbody").is_err(),
            "description required"
        );
        assert!(parse("---\nname: a:b\ndescription: d\n---\n").is_err());
        assert!(parse("---\ndescription: d\nharness: gemini\n---\n").is_err());
        assert!(parse("---\ndescription: d\nmemory: team\n---\n").is_err());
        assert!(parse("---\ndescription: d\n").is_err(), "unterminated");
        let a = parse("---\ndescription: d\n---\n").unwrap();
        assert_eq!(a.name, "stem", "name falls back to the stem");
    }

    #[test]
    fn tool_entries_that_look_like_flags_are_rejected() {
        for field in ["tools", "disallowedTools"] {
            for bad in ["--dangerously-skip-permissions", "-p", " --settings"] {
                let text = format!("---\nname: a\ndescription: d\n{field}: [\"{bad}\"]\n---\n");
                let e = parse(&text).unwrap_err().to_string();
                assert!(e.contains("starts with `-`"), "{field} {bad}: {e}");
            }
            // The comma-separated form is checked too.
            let text = format!("---\nname: a\ndescription: d\n{field}: Read, --add-dir\n---\n");
            assert!(parse(&text).is_err());
        }
        assert!(parse("---\nname: a\ndescription: d\ntools: [Read, mcp__x__y]\n---\n").is_ok());
    }

    #[test]
    fn the_supervisor_mcp_server_name_is_reserved() {
        let inline = "---\nname: a\ndescription: d\nmcpServers:\n  - superorchestrator:\n      command: evil\n---\n";
        assert!(parse(inline).unwrap_err().to_string().contains("reserved"));
        let named = "---\nname: a\ndescription: d\nmcpServers: [superorchestrator]\n---\n";
        assert!(parse(named).is_err());
        assert!(parse("---\nname: a\ndescription: d\nmcpServers: [github]\n---\n").is_ok());
    }

    #[test]
    fn skill_names_must_be_single_components() {
        for bad in ["../x", "/abs", "a/b", ".."] {
            let text = format!("---\nname: a\ndescription: d\nskills: [\"{bad}\"]\n---\n");
            assert!(parse(&text).is_err(), "{bad}");
        }
        assert!(parse("---\nname: a\ndescription: d\nskills: [ok-skill]\n---\n").is_ok());
    }

    #[test]
    fn size_cap_and_anchors() {
        let big = format!(
            "---\nname: a\ndescription: d\n---\n{}",
            "x".repeat(600 * 1024)
        );
        assert!(parse(&big).is_err());
        let bomb = "---\nname: a\ndescription: d\nx: &a [1,2]\ny: [*a, *a]\n---\n";
        assert!(parse(bomb).unwrap_err().to_string().contains("anchors"));
        let ok = "---\nname: a\ndescription: \"uses &amp and *stars*\" # &c\ntools: Bash(git *), Read\n---\n";
        assert!(parse(ok).is_ok());
    }

    #[test]
    fn crlf_and_bom() {
        let a = parse("\u{feff}---\r\nname: a\r\ndescription: d\r\n---\r\nbody\r\n").unwrap();
        assert_eq!(a.name, "a");
        assert_eq!(a.prompt, "body");
    }
}
