//! `levels list|add|edit|remove|reset|choice`: edits `router.levels` (and `[[choices]]` for
//! `choice add --harness`) in config.toml. Local only; the supervisor reads it on start.

use anyhow::{Result, bail};
use dialoguer::Confirm;
use serde_json::{Value, json};
use so_core::config::{Level, LevelChoice, LevelTier};
use so_core::{Choice, Config, Harness};
use so_router::Catalog;
use so_web::types::LevelsView;

use crate::cli::{LevelAdd, LevelChoiceAdd, LevelChoiceCmd, LevelEdit, LevelsCmd};
use crate::ctx::{Ctx, interactive};
use crate::output::write_table;

const LEVELS_KEY: &str = "router.levels";
const RESTART: &str = "restart the supervisor to apply";

pub fn run(ctx: &Ctx, cmd: LevelsCmd) -> Result<()> {
    match cmd {
        LevelsCmd::List => list(ctx),
        LevelsCmd::Add(a) => add(ctx, a),
        LevelsCmd::Edit(a) => edit(ctx, a),
        LevelsCmd::Remove { name } => remove(ctx, &name),
        LevelsCmd::Reset => reset(ctx),
        LevelsCmd::Choice(LevelChoiceCmd::Add(a)) => choice_add(ctx, a),
        LevelsCmd::Choice(LevelChoiceCmd::Set {
            level,
            choice,
            priority,
        }) => choice_set(ctx, &level, &choice, priority),
        LevelsCmd::Choice(LevelChoiceCmd::Remove { level, choice }) => {
            choice_remove(ctx, &level, &choice)
        }
    }
}

fn tier_of(s: &str) -> Result<LevelTier> {
    Ok(match s {
        "high" => LevelTier::High,
        "mid" => LevelTier::Mid,
        "fast" => LevelTier::Fast,
        other => bail!("unknown tier `{other}` (high|mid|fast)"),
    })
}

fn tier_name(t: LevelTier) -> &'static str {
    match t {
        LevelTier::High => "high",
        LevelTier::Mid => "mid",
        LevelTier::Fast => "fast",
    }
}

fn state(on: Option<bool>) -> &'static str {
    match on {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    }
}

// ------------------------------------------------------------------ list

fn list(ctx: &Ctx) -> Result<()> {
    let cfg = ctx.config()?;
    let defaults = !Config::file_sets_key(&ctx.paths, LEVELS_KEY)?;
    let view = LevelsView::resolve(&cfg, defaults, false);
    // A level with no listed choices takes the enabled worker choices of its tier.
    let catalog = match Catalog::load(&ctx.paths) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("warning: model catalog unavailable ({e}); tier levels cannot be resolved");
            None
        }
    };
    let tier_choices = |l: &Level| -> Vec<&Choice> {
        let (Some(tier), Some(catalog), true) = (l.tier, catalog.as_ref(), l.choices.is_empty())
        else {
            return Vec::new();
        };
        let want = so_router::levels::tier_of(tier);
        cfg.choices
            .iter()
            .filter(|c| c.enabled && c.roles.iter().any(|r| r == "worker"))
            .filter(|c| catalog.tier(c.harness, &c.model) == want)
            .collect()
    };
    let levels: Vec<Value> = view
        .levels
        .iter()
        .zip(&cfg.router.levels)
        .map(|(v, l)| {
            let mut o = json!({
                "name": v.name,
                "min": v.min,
                "max": v.max,
                "tier": v.tier.map(tier_name),
                "choices": v.choices.iter().map(|c| json!({
                    "id": c.id,
                    "priority": c.priority,
                    "harness": c.harness.map(|h| h.as_str()),
                    "model": c.model,
                    "effort": c.effort,
                    "enabled": c.enabled,
                    "known": c.known,
                    "state": state(c.known.then_some(c.enabled)),
                })).collect::<Vec<_>>(),
            });
            if v.choices.is_empty() {
                o["tier_choices"] = if catalog.is_some() {
                    tier_choices(l).iter().map(|c| json!(c.id)).collect()
                } else {
                    Value::Null
                };
            }
            o
        })
        .collect();
    ctx.emit(
        &json!({ "default_levels": defaults, "levels": levels }),
        |w| {
            if defaults {
                writeln!(
                    w,
                    "(defaults) no router.levels in config.toml; these tier defaults are in effect"
                )?;
            } else if view.levels.is_empty() {
                return writeln!(w, "no levels: complexity is not mapped to a level");
            }
            let mut rows = Vec::new();
            for (v, l) in view.levels.iter().zip(&cfg.router.levels) {
                let head =
                    |priority: String, id: String, h: String, m: String, e: String, s: &str| {
                        vec![
                            v.name.clone(),
                            format!("{}-{}", v.min, v.max),
                            priority,
                            id,
                            h,
                            m,
                            e,
                            s.to_string(),
                        ]
                    };
                if !v.choices.is_empty() {
                    for c in &v.choices {
                        rows.push(head(
                            c.priority.map_or("-".into(), |p| p.to_string()),
                            c.id.clone(),
                            c.harness.map_or("-".into(), |h| h.to_string()),
                            c.model.clone().unwrap_or("-".into()),
                            c.effort.clone().unwrap_or("-".into()),
                            state(c.known.then_some(c.enabled)),
                        ));
                    }
                    continue;
                }
                let tier = v.tier.map_or("-", tier_name);
                let found = tier_choices(l);
                if found.is_empty() {
                    let why = if catalog.is_some() {
                        "no enabled choice"
                    } else {
                        "catalog unavailable"
                    };
                    rows.push(head(
                        "-".into(),
                        format!("(tier {tier}: {why})"),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        "-",
                    ));
                }
                for c in found {
                    rows.push(head(
                        "-".into(),
                        format!("{} (tier {tier})", c.id),
                        c.harness.to_string(),
                        c.model.clone(),
                        c.effort.clone().unwrap_or("-".into()),
                        "on",
                    ));
                }
            }
            write_table(
                w,
                &[
                    "LEVEL", "RANGE", "PRIORITY", "CHOICE", "HARNESS", "MODEL", "EFFORT", "STATE",
                ],
                &rows,
            )
        },
    )
}

// ------------------------------------------------------------------ editing

/// The levels as they will be written, plus whether the file had to be given the defaults.
struct Draft {
    cfg: Config,
    levels: Vec<Level>,
    choices: Option<Vec<Choice>>,
    materialised: bool,
}

impl Draft {
    /// With no `router.levels` in the file, the defaults in effect become the starting point.
    fn load(ctx: &Ctx) -> Result<Self> {
        let cfg = ctx.config()?;
        Ok(Self {
            levels: cfg.router.levels.clone(),
            materialised: !Config::file_sets_key(&ctx.paths, LEVELS_KEY)?,
            choices: None,
            cfg,
        })
    }

    fn level(&mut self, name: &str) -> Result<&mut Level> {
        self.levels
            .iter_mut()
            .find(|l| l.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown level `{name}`"))
    }

    /// Validates and writes `router.levels` (and `choices` if changed) in one write.
    fn save(self, ctx: &Ctx, done: &str) -> Result<()> {
        let levels = toml::Value::try_from(&self.levels)?.to_string();
        let choices = self
            .choices
            .as_ref()
            .map(toml::Value::try_from)
            .transpose()?
            .map(|v| v.to_string());
        let mut entries = Vec::new();
        if let Some(c) = &choices {
            entries.push(("choices", c.as_str()));
        }
        entries.push((LEVELS_KEY, levels.as_str()));
        Config::set_many_in_file(&ctx.paths, &entries)?;
        ctx.refresh_skill();
        let mut line = String::new();
        if self.materialised {
            let names: Vec<_> = self
                .cfg
                .router
                .levels
                .iter()
                .map(|l| l.name.as_str())
                .collect();
            line.push_str(&format!(
                "config.toml had no router.levels; wrote the current defaults first ({})\n",
                names.join(", ")
            ));
        }
        line.push_str(&format!("{done}\n{RESTART}"));
        ctx.emit_ok(
            json!({
                "levels": self.levels,
                "materialised_defaults": self.materialised,
                "restart_required": true,
            }),
            &line,
        )
    }
}

fn add(ctx: &Ctx, a: LevelAdd) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    let tier = a.tier.as_deref().map(tier_of).transpose()?;
    if tier.is_none() {
        bail!("a new level needs --tier (it lists no choices yet)");
    }
    d.levels.push(Level {
        name: a.name.clone(),
        min: a.min,
        max: a.max,
        tier,
        choices: Vec::new(),
    });
    d.save(
        ctx,
        &format!("added level {} ({}-{})", a.name, a.min, a.max),
    )
}

fn edit(ctx: &Ctx, a: LevelEdit) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    let tier = a.tier.as_deref().map(tier_of).transpose()?;
    if a.new_name.is_none() && a.min.is_none() && a.max.is_none() && tier.is_none() && !a.no_tier {
        bail!("nothing to change; pass --name, --min, --max, --tier or --no-tier");
    }
    let l = d.level(&a.name)?;
    if let Some(n) = a.new_name {
        l.name = n;
    }
    if let Some(v) = a.min {
        l.min = v;
    }
    if let Some(v) = a.max {
        l.max = v;
    }
    if tier.is_some() {
        l.tier = tier;
    }
    if a.no_tier {
        l.tier = None;
    }
    let (new, min, max) = (l.name.clone(), l.min, l.max);
    d.save(ctx, &format!("updated level {new} ({min}-{max})"))
}

fn remove(ctx: &Ctx, name: &str) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    d.level(name)?;
    d.levels.retain(|l| l.name != name);
    let msg = if d.levels.is_empty() {
        format!(
            "removed level {name}\nno levels left; levels are off, use `levels reset` for the defaults"
        )
    } else {
        format!("removed level {name}")
    };
    d.save(ctx, &msg)
}

fn reset(ctx: &Ctx) -> Result<()> {
    if !Config::file_sets_key(&ctx.paths, LEVELS_KEY)? {
        return ctx.emit_ok(
            json!({ "changed": false }),
            "config.toml has no router.levels; the defaults are already in effect",
        );
    }
    if !ctx.yes {
        if !interactive() {
            bail!("refusing to reset levels without confirmation; pass --yes");
        }
        if !Confirm::new()
            .with_prompt("remove router.levels and go back to the default levels?")
            .default(false)
            .interact()?
        {
            bail!("nothing changed");
        }
    }
    Config::unset_in_file(&ctx.paths, LEVELS_KEY)?;
    ctx.emit_ok(
        json!({ "changed": true, "restart_required": true }),
        &format!("removed router.levels; the default levels apply again\n{RESTART}"),
    )
}

// ------------------------------------------------------------------ choices

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn choice_add(ctx: &Ctx, a: LevelChoiceAdd) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    d.level(&a.level)?;
    let id = match (&a.choice, &a.harness, &a.model) {
        (Some(id), None, None) => {
            if !d.cfg.choices.iter().any(|c| &c.id == id) {
                bail!("unknown choice `{id}`; create one with --harness and --model");
            }
            id.clone()
        }
        (None, Some(h), Some(m)) => {
            let harness: Harness = h.parse()?;
            if m.trim().is_empty() {
                bail!("--model must not be empty");
            }
            if a.effort.as_deref().is_some_and(|e| e.trim().is_empty()) {
                bail!("--effort must not be empty (omit it for no effort)");
            }
            let effort = a.effort.clone();
            if let Some(e) = &effort
                && let Some(problem) = harness.model_effort_problem(m, e)
            {
                bail!("{problem}");
            }
            let id = a.id.clone().unwrap_or_else(|| {
                let mut parts = vec![harness.as_str().to_string(), slug(m)];
                parts.extend(effort.as_deref().map(slug));
                parts.join("-")
            });
            if id.trim().is_empty() || id.trim_end_matches('-') == harness.as_str() {
                bail!(
                    "the choice id `{id}` is empty or only the harness name; pass --id or a --model with letters or digits"
                );
            }
            if d.cfg.choices.iter().any(|c| c.id == id) {
                bail!(
                    "choice `{id}` already exists; add it with `levels choice add {} {id}` or pick another --id",
                    a.level
                );
            }
            let roles = if a.role.is_empty() {
                vec!["worker".to_string()]
            } else {
                a.role.clone()
            };
            let mut choices = d.cfg.choices.clone();
            choices.push(Choice {
                id: id.clone(),
                harness,
                model: m.clone(),
                effort,
                roles,
                description: "Added by `levels choice add`.".to_string(),
                enabled: true,
            });
            d.choices = Some(choices);
            id
        }
        _ => bail!("give a choice id, or --harness and --model for a new choice"),
    };
    let created = d.choices.is_some();
    let level = a.level.clone();
    let was_tier = {
        let l = d.level(&level)?;
        l.choices.is_empty().then_some(l.tier).flatten()
    };
    d.level(&level)?.choices.push(LevelChoice {
        id: id.clone(),
        priority: a.priority,
    });
    let mut msg = if created {
        format!("created choice {id} and added it to level {level}")
    } else {
        format!("added choice {id} to level {level}")
    };
    if let Some(t) = was_tier {
        msg.push_str(&format!(
            "\nlevel {level} now uses its listed choices; tier {} no longer applies",
            tier_name(t)
        ));
    }
    d.save(ctx, &msg)
}

fn choice_set(ctx: &Ctx, level: &str, choice: &str, priority: u32) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    let l = d.level(level)?;
    let Some(c) = l.choices.iter_mut().find(|c| c.id == choice) else {
        bail!("level `{level}` does not list choice `{choice}`");
    };
    c.priority = Some(priority);
    d.save(
        ctx,
        &format!("set priority of {choice} in level {level} to {priority}"),
    )
}

fn choice_remove(ctx: &Ctx, level: &str, choice: &str) -> Result<()> {
    let mut d = Draft::load(ctx)?;
    let l = d.level(level)?;
    let before = l.choices.len();
    l.choices.retain(|c| c.id != choice);
    if l.choices.len() == before {
        bail!("level `{level}` does not list choice `{choice}`");
    }
    d.save(ctx, &format!("removed choice {choice} from level {level}"))
}
