//! `levels list|add|edit|remove|reset|model`: edits `router.levels` in config.toml. Local
//! only; the supervisor reads it on start.

use anyhow::{Result, bail};
use dialoguer::Confirm;
use serde_json::json;
use so_core::config::{Level, LevelTier};
use so_core::{AccountFile, Choice, Config};
use so_router::Catalog;

use crate::cli::{LevelAdd, LevelEdit, LevelModelCmd, LevelsCmd};
use crate::ctx::{Ctx, interactive};
use crate::output::write_table;

const LEVELS_KEY: &str = "router.levels";
const RESTART: &str = "restart the supervisor to apply";

pub fn run(ctx: &Ctx, cmd: LevelsCmd) -> Result<()> {
    if matches!(cmd, LevelsCmd::List) {
        return list(ctx);
    }
    if matches!(cmd, LevelsCmd::Reset) && Config::file_sets_key(&ctx.paths, LEVELS_KEY)? && !ctx.yes
    {
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
    let result = apply(ctx, cmd)?;
    if result.changed {
        ctx.refresh_skill();
    }
    ctx.emit_ok(result.value, &result.text)
}

/// Silent editing path shared with the TUI. Callers confirm destructive actions first.
/// Config writes validate the entire draft before replacing the file.
pub(crate) struct EditResult {
    pub value: serde_json::Value,
    pub text: String,
    pub changed: bool,
}

pub(crate) fn apply(ctx: &Ctx, cmd: LevelsCmd) -> Result<EditResult> {
    match cmd {
        LevelsCmd::List => bail!("list is not a level edit"),
        LevelsCmd::Add(a) => add(ctx, a),
        LevelsCmd::Edit(a) => edit(ctx, a),
        LevelsCmd::Remove { name } => remove(ctx, &name),
        LevelsCmd::Reset => reset(ctx),
        LevelsCmd::Model(LevelModelCmd::Add {
            level,
            model,
            position,
        }) => model_add(ctx, &level, &model, position),
        LevelsCmd::Model(LevelModelCmd::Move {
            level,
            model,
            position,
        }) => model_move(ctx, &level, &model, position),
        LevelsCmd::Model(LevelModelCmd::Remove { level, model }) => {
            model_remove(ctx, &level, &model)
        }
    }
}

/// `on` when the catalog lists the model and an enabled account serves its harness, `off`
/// when it is listed but no enabled account serves the harness, `unknown` when the catalog
/// does not list it (or could not be read).
fn state(catalog: Option<&Catalog>, accounts: &[AccountFile], c: &Choice) -> &'static str {
    match catalog {
        Some(cat) if cat.contains(c.harness, &c.model) => {
            if accounts.iter().any(|a| a.enabled && a.harness == c.harness) {
                "on"
            } else {
                "off"
            }
        }
        _ => "unknown",
    }
}

// ------------------------------------------------------------------ list

fn list(ctx: &Ctx) -> Result<()> {
    let cfg = ctx.config()?;
    let defaults = !Config::file_sets_key(&ctx.paths, LEVELS_KEY)?;
    let catalog = match Catalog::load(&ctx.paths) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("warning: model catalog unavailable ({e}); model states are unknown");
            None
        }
    };
    let accounts = AccountFile::list(&ctx.paths)?;
    let levels: Vec<_> = cfg
        .router
        .levels
        .iter()
        .map(|l| {
            json!({
                "name": l.name,
                "min": l.min,
                "max": l.max,
                "tier": l.tier.map(LevelTier::as_str),
                "models": l.models.iter().enumerate().map(|(i, c)| json!({
                    "position": i + 1,
                    "ref": c.id(),
                    "harness": c.harness.as_str(),
                    "model": c.model,
                    "effort": c.effort,
                    "state": state(catalog.as_ref(), &accounts, c),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    ctx.emit(
        &json!({ "default_levels": defaults, "levels": levels }),
        |w| {
            if defaults {
                writeln!(
                    w,
                    "(defaults) no router.levels in config.toml; these default levels are in effect"
                )?;
            } else if cfg.router.levels.is_empty() {
                return writeln!(w, "no levels: complexity is not mapped to a level");
            }
            let mut rows = Vec::new();
            for l in &cfg.router.levels {
                let range = format!("{}-{}", l.min, l.max);
                if l.models.is_empty() {
                    let tier = l.tier.map_or("-", LevelTier::as_str);
                    rows.push(vec![
                        l.name.clone(),
                        range,
                        "-".into(),
                        format!("(tier {tier}: no models listed)"),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                    ]);
                    continue;
                }
                for (i, c) in l.models.iter().enumerate() {
                    rows.push(vec![
                        l.name.clone(),
                        range.clone(),
                        (i + 1).to_string(),
                        c.id(),
                        c.harness.to_string(),
                        c.model.clone(),
                        c.effort.clone().unwrap_or("-".into()),
                        state(catalog.as_ref(), &accounts, c).to_string(),
                    ]);
                }
            }
            write_table(
                w,
                &[
                    "LEVEL",
                    "RANGE",
                    "POSITION",
                    "MODEL REF",
                    "HARNESS",
                    "MODEL",
                    "EFFORT",
                    "STATE",
                ],
                &rows,
            )
        },
    )
}

// ------------------------------------------------------------------ editing

/// The levels as they will be written, plus whether the file had to be given the defaults.
struct Draft {
    levels: Vec<Level>,
    materialised: bool,
}

impl Draft {
    /// With no `router.levels` in the file, the defaults in effect become the starting point.
    fn load(ctx: &Ctx) -> Result<Self> {
        let cfg = ctx.config()?;
        Ok(Self {
            levels: cfg.router.levels,
            materialised: !Config::file_sets_key(&ctx.paths, LEVELS_KEY)?,
        })
    }

    fn level(&mut self, name: &str) -> Result<&mut Level> {
        self.levels
            .iter_mut()
            .find(|l| l.name == name)
            .ok_or_else(|| anyhow::anyhow!("unknown level `{name}`"))
    }

    /// Validates and writes `router.levels` in one write.
    fn save(self, ctx: &Ctx, done: &str) -> Result<EditResult> {
        let levels = toml::Value::try_from(&self.levels)?.to_string();
        Config::set_many_in_file(&ctx.paths, &[(LEVELS_KEY, levels.as_str())])?;
        let mut line = String::new();
        if self.materialised {
            line.push_str("config.toml had no router.levels; wrote the current defaults first\n");
        }
        line.push_str(&format!("{done}\n{RESTART}"));
        Ok(EditResult {
            value: json!({
                "levels": self.levels,
                "materialised_defaults": self.materialised,
                "restart_required": true,
            }),
            text: line,
            changed: true,
        })
    }
}

/// A level name without surrounding whitespace; an empty one is an error.
fn clean_name(s: &str) -> Result<String> {
    let name = s.trim();
    if name.is_empty() {
        bail!("level name must not be empty");
    }
    Ok(name.to_string())
}

fn add(ctx: &Ctx, a: LevelAdd) -> Result<EditResult> {
    let name = clean_name(&a.name)?;
    let mut d = Draft::load(ctx)?;
    let tier = a
        .tier
        .as_deref()
        .map(str::parse::<LevelTier>)
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let models = a
        .models
        .iter()
        .map(|m| m.parse::<Choice>().map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?;
    if tier.is_none() && models.is_empty() {
        bail!("a new level needs --tier or at least one --model");
    }
    d.levels.push(Level {
        name: name.clone(),
        min: a.min,
        max: a.max,
        tier,
        models,
    });
    d.save(ctx, &format!("added level {name} ({}-{})", a.min, a.max))
}

fn edit(ctx: &Ctx, a: LevelEdit) -> Result<EditResult> {
    let mut d = Draft::load(ctx)?;
    let tier = a
        .tier
        .as_deref()
        .map(str::parse::<LevelTier>)
        .transpose()
        .map_err(anyhow::Error::msg)?;
    if a.new_name.is_none() && a.min.is_none() && a.max.is_none() && tier.is_none() && !a.no_tier {
        bail!("nothing to change; pass --name, --min, --max, --tier or --no-tier");
    }
    let new_name = a.new_name.as_deref().map(clean_name).transpose()?;
    let l = d.level(&a.name)?;
    if let Some(n) = new_name {
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

fn remove(ctx: &Ctx, name: &str) -> Result<EditResult> {
    let mut d = Draft::load(ctx)?;
    d.level(name)?;
    if d.levels.len() == 1 {
        bail!(
            "refusing to remove the last level: nothing would be routable; use `levels reset` for the defaults"
        );
    }
    d.levels.retain(|l| l.name != name);
    d.save(ctx, &format!("removed level {name}"))
}

fn reset(ctx: &Ctx) -> Result<EditResult> {
    if !Config::file_sets_key(&ctx.paths, LEVELS_KEY)? {
        return Ok(EditResult {
            value: json!({ "changed": false }),
            text: "config.toml has no router.levels; the defaults are already in effect".into(),
            changed: false,
        });
    }
    Config::unset_in_file(&ctx.paths, LEVELS_KEY)?;
    Ok(EditResult {
        value: json!({ "changed": true, "restart_required": true }),
        text: format!("removed router.levels; the default levels apply again\n{RESTART}"),
        changed: true,
    })
}

// ------------------------------------------------------------------ models

/// Index of `target` in the level's model list.
fn index_of(l: &Level, target: &Choice) -> Result<usize> {
    l.models
        .iter()
        .position(|c| c == target)
        .ok_or_else(|| anyhow::anyhow!("level `{}` does not list model `{}`", l.name, target.id()))
}

/// A 1-based position as a 0-based index, at most `max` (the list length, or one more to
/// append).
fn slot(position: usize, max: usize) -> Result<usize> {
    if position == 0 || position > max {
        bail!("--position must be between 1 and {max}");
    }
    Ok(position - 1)
}

fn model_add(ctx: &Ctx, level: &str, model: &str, position: Option<usize>) -> Result<EditResult> {
    let choice: Choice = model.parse()?;
    let mut d = Draft::load(ctx)?;
    let l = d.level(level)?;
    if l.models.contains(&choice) {
        bail!("level `{level}` already lists model `{}`", choice.id());
    }
    let at = match position {
        Some(p) => slot(p, l.models.len() + 1)?,
        None => l.models.len(),
    };
    let was_tier = l.models.is_empty().then_some(l.tier).flatten();
    let id = choice.id();
    l.models.insert(at, choice);
    let mut msg = format!("added model {id} to level {level} at position {}", at + 1);
    if let Some(t) = was_tier {
        msg.push_str(&format!(
            "\nlevel {level} now uses its listed models; tier {} no longer applies",
            LevelTier::as_str(t)
        ));
    }
    d.save(ctx, &msg)
}

fn model_move(ctx: &Ctx, level: &str, model: &str, position: usize) -> Result<EditResult> {
    let choice: Choice = model.parse()?;
    let mut d = Draft::load(ctx)?;
    let l = d.level(level)?;
    let from = index_of(l, &choice)?;
    let to = slot(position, l.models.len())?;
    let c = l.models.remove(from);
    l.models.insert(to, c);
    d.save(
        ctx,
        &format!(
            "moved model {} in level {level} to position {}",
            choice.id(),
            to + 1
        ),
    )
}

fn model_remove(ctx: &Ctx, level: &str, model: &str) -> Result<EditResult> {
    let choice: Choice = model.parse()?;
    let mut d = Draft::load(ctx)?;
    let l = d.level(level)?;
    let at = index_of(l, &choice)?;
    l.models.remove(at);
    d.save(
        ctx,
        &format!("removed model {} from level {level}", choice.id()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_edits_persist_order_validate_atomically_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(dir.path().to_path_buf()), false, false).unwrap();
        let original = "# keep this setting\n[limits]\nglobal_cap = 7\n\n[[router.levels]]\nname = 'one'\nmin = 0\nmax = 20\nmodels = ['codex/gpt-6.1-sol/high']\n\n[[router.levels]]\nname = 'two'\nmin = 50\nmax = 100\ntier = 'fast'\n";
        std::fs::write(ctx.paths.config_toml(), original).unwrap();
        let result = apply(
            &ctx,
            LevelsCmd::Add(LevelAdd {
                name: "gap".into(),
                min: 21,
                max: 49,
                tier: None,
                models: vec!["agy/claude-sonnet-4-6/thinking".into()],
            }),
        )
        .unwrap();
        assert!(result.changed && result.text.contains(RESTART));
        let model = "codex/gpt-6.1-sol/medium";
        apply(
            &ctx,
            LevelsCmd::Model(LevelModelCmd::Add {
                level: "gap".into(),
                model: model.into(),
                position: None,
            }),
        )
        .unwrap();
        apply(
            &ctx,
            LevelsCmd::Model(LevelModelCmd::Move {
                level: "gap".into(),
                model: model.into(),
                position: 1,
            }),
        )
        .unwrap();
        let config = ctx.config().unwrap();
        assert_eq!(config.router.levels[2].models[0].id(), model);
        let before = std::fs::read_to_string(ctx.paths.config_toml()).unwrap();
        for (min, max) in [(20, 49), (21, 101), (49, 21)] {
            let err = apply(
                &ctx,
                LevelsCmd::Edit(LevelEdit {
                    name: "gap".into(),
                    new_name: None,
                    min: Some(min),
                    max: Some(max),
                    tier: None,
                    no_tier: false,
                }),
            );
            assert!(err.is_err());
            assert_eq!(
                std::fs::read_to_string(ctx.paths.config_toml()).unwrap(),
                before
            );
        }
        apply(
            &ctx,
            LevelsCmd::Model(LevelModelCmd::Remove {
                level: "gap".into(),
                model: model.into(),
            }),
        )
        .unwrap();
        apply(&ctx, LevelsCmd::Remove { name: "gap".into() }).unwrap();
        let result = apply(&ctx, LevelsCmd::Reset).unwrap();
        assert!(result.changed && result.text.contains(RESTART));
        assert!(!Config::file_sets_key(&ctx.paths, LEVELS_KEY).unwrap());
        assert_eq!(ctx.config().unwrap().limits.global_cap, 7);
        assert!(
            std::fs::read_to_string(ctx.paths.config_toml())
                .unwrap()
                .contains("# keep this setting")
        );
        assert!(!apply(&ctx, LevelsCmd::Reset).unwrap().changed);
    }
}
