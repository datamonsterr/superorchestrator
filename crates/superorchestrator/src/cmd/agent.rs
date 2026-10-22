//! `agent create|list|show|edit|remove` (SPEC 10, 15).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dialoguer::Confirm;
use serde_json::json;
use so_agents::{
    AgentDef, AgentError, HarnessPref, discover, gaps, resolve_for_spawn, validate_agent_name,
};
use so_core::Harness;

use crate::cli::{AgentCmd, AgentCreate};
use crate::ctx::{Ctx, interactive};
use crate::exit::CliError;
use crate::output::write_table;

pub fn run(ctx: &Ctx, cmd: AgentCmd) -> Result<()> {
    match cmd {
        AgentCmd::Create(a) => create(ctx, a),
        AgentCmd::List => list(ctx),
        AgentCmd::Show { name } => show(ctx, &name),
        AgentCmd::Edit { name } => edit(ctx, &name),
        AgentCmd::Remove { name } => remove(ctx, &name),
    }
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().context("could not read the current directory")
}

fn find(ctx: &Ctx, name: &str) -> Result<AgentDef> {
    match resolve_for_spawn(&cwd()?, &ctx.paths, name) {
        Err(AgentError::NotFound(n)) => {
            Err(CliError::not_found(format!("no agent named `{n}`")).into())
        }
        other => Ok(other?),
    }
}

fn scaffold(name: &str, harness: &str, description: &str) -> String {
    format!(
        "---\nname: \"{name}\"\ndescription: {description}\nharness: {harness}\n\
# accounts: [codex-acc2]\n# model:\n# effort: medium\n# tools: [Read, Grep, Bash]\n\
# disallowedTools: [Write]\n# skills: []\n# mcpServers: []\n# memory: project\n\
# maxTurns: 40\n# isolation: worktree\n# review: false\n---\n\
Describe how this agent should work.\n"
    )
}

fn create(ctx: &Ctx, a: AgentCreate) -> Result<()> {
    validate_agent_name(&a.name)?;
    let harness = match a.harness.as_deref() {
        None => HarnessPref::Auto,
        Some(h) => HarnessPref::parse(h).map_err(|m| anyhow::anyhow!(m))?,
    };
    let dir = ctx.paths.agents_dir();
    let folder = dir.join(&a.name);
    let file = folder.join("AGENT.md");
    if file.exists() || dir.join(format!("{}.md", a.name)).exists() {
        bail!("agent `{}` already exists in {}", a.name, dir.display());
    }
    let description = a
        .description
        .unwrap_or_else(|| format!("TODO: when to use {}", a.name));
    // Quote so a description with `:` or `#` stays valid YAML.
    let quoted = serde_json::to_string(&description)?;
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("could not create {}", folder.display()))?;
    std::fs::write(&file, scaffold(&a.name, harness.as_str(), &quoted))
        .with_context(|| format!("could not write {}", file.display()))?;
    ctx.emit_ok(
        json!({ "name": a.name, "path": file }),
        &format!("created {}", file.display()),
    )?;
    if interactive() && !ctx.json {
        open_editor(&file)?;
    }
    Ok(())
}

fn list(ctx: &Ctx) -> Result<()> {
    let d = discover(&cwd()?, &ctx.paths);
    for (path, msg) in &d.problems {
        eprintln!("warning: skipped {}: {msg}", path.display());
    }
    let items: Vec<_> = d
        .agents
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "source": a.source.as_str(),
                "harness": a.harness.as_str(),
                "description": a.description,
                "path": a.file,
            })
        })
        .collect();
    ctx.emit(&items, |w| {
        if d.agents.is_empty() {
            return writeln!(
                w,
                "no agents; run `superorchestrator init` or `agent create <name>`"
            );
        }
        let rows: Vec<Vec<String>> = d
            .agents
            .iter()
            .map(|a| {
                vec![
                    a.name.clone(),
                    a.source.as_str().into(),
                    a.harness.as_str().into(),
                    a.description.clone(),
                ]
            })
            .collect();
        write_table(w, &["NAME", "SOURCE", "HARNESS", "DESCRIPTION"], &rows)
    })
}

const ALL: [Harness; 4] = [
    Harness::Claude,
    Harness::Codex,
    Harness::Opencode,
    Harness::Agy,
];

fn show(ctx: &Ctx, name: &str) -> Result<()> {
    let a = find(ctx, name)?;
    let per: Vec<_> = ALL.iter().map(|h| (*h, gaps(&a, *h))).collect();
    let gap_json: serde_json::Map<String, serde_json::Value> = per
        .iter()
        .map(|(h, g)| (h.as_str().to_string(), json!(g)))
        .collect();
    let mut v = serde_json::to_value(&a)?;
    v["gaps"] = gap_json.into();
    ctx.emit(&v, |w| {
        writeln!(w, "name:        {}", a.name)?;
        writeln!(
            w,
            "source:      {} ({})",
            a.source.as_str(),
            a.file.display()
        )?;
        writeln!(w, "description: {}", a.description)?;
        writeln!(w, "harness:     {}", a.harness.as_str())?;
        let opt = |o: &Option<String>| o.clone().unwrap_or_else(|| "-".into());
        writeln!(w, "model:       {}", opt(&a.model))?;
        writeln!(w, "effort:      {}", opt(&a.effort))?;
        let list = |l: &[String]| {
            if l.is_empty() {
                "-".into()
            } else {
                l.join(", ")
            }
        };
        writeln!(w, "accounts:    {}", list(&a.accounts))?;
        writeln!(w, "tools:       {}", list(&a.tools))?;
        writeln!(w, "disallowed:  {}", list(&a.disallowed_tools))?;
        writeln!(w, "skills:      {}", list(&a.skills))?;
        let mcp: Vec<String> = a.mcp_servers.iter().map(|m| m.name().to_string()).collect();
        writeln!(w, "mcpServers:  {}", list(&mcp))?;
        writeln!(
            w,
            "memory:      {}",
            a.memory
                .map_or("-".into(), |m| format!("{m:?}").to_lowercase())
        )?;
        writeln!(
            w,
            "maxTurns:    {}",
            a.max_turns.map_or("-".into(), |n| n.to_string())
        )?;
        writeln!(
            w,
            "isolation:   {}",
            a.isolation.map_or("-", |i| i.as_str())
        )?;
        writeln!(w, "review:      {}", a.review)?;
        writeln!(w, "\ngaps (what each harness cannot honour):")?;
        for (h, g) in &per {
            if g.is_empty() {
                writeln!(w, "  {}: none", h.as_str())?;
            } else {
                writeln!(w, "  {}:", h.as_str())?;
                for line in g {
                    writeln!(w, "    - {line}")?;
                }
            }
        }
        Ok(())
    })
}

fn editable(a: &AgentDef, verb: &str) -> Result<()> {
    if a.source.read_only() {
        bail!(
            "`{}` is a read-only import from {}; copy it to the config agents dir to {verb} it",
            a.name,
            a.file.display()
        );
    }
    Ok(())
}

fn open_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    // $EDITOR may carry arguments ("code --wait"); split on whitespace.
    let mut parts = editor.split_whitespace();
    let program = parts.next().context("empty $EDITOR")?;
    let status = Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("could not start editor `{editor}`"))?;
    if !status.success() {
        bail!("editor `{editor}` exited with {status}");
    }
    Ok(())
}

fn edit(ctx: &Ctx, name: &str) -> Result<()> {
    let a = find(ctx, name)?;
    editable(&a, "edit")?;
    open_editor(&a.file)?;
    // Re-parse so a broken edit is reported right away.
    let re = find(ctx, name)
        .with_context(|| format!("{} is invalid after editing", a.file.display()))?;
    ctx.emit_ok(
        json!({ "name": re.name, "path": re.file }),
        &format!("{} is valid", re.name),
    )
}

fn remove(ctx: &Ctx, name: &str) -> Result<()> {
    let a = find(ctx, name)?;
    editable(&a, "remove")?;
    if !ctx.yes {
        if !interactive() {
            bail!("refusing to remove `{name}` without confirmation; pass --yes");
        }
        if !Confirm::new()
            .with_prompt(format!("remove agent `{name}` ({})?", a.file.display()))
            .default(false)
            .interact()?
        {
            bail!("nothing removed");
        }
    }
    so_agents::remove_files(&a)?;
    ctx.emit_ok(json!({ "name": name }), &format!("removed {name}"))
}
