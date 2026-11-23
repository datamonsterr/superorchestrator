use super::super::app::{App, Effect};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    text::Line,
    widgets::Paragraph,
};
use so_supervisor::services::decider::{DeciderTestResult, DeciderView};

const NAMES: [&str; 10] = [
    "endpoint",
    "model",
    "fallback_model",
    "on_unreachable",
    "timeout",
    "min_confidence",
    "min_margin",
    "key env",
    "key file",
    "key paste",
];

/// Holds a possible secret without exposing it to Debug output.
#[derive(Clone, Default)]
pub struct Patch(pub serde_json::Map<String, serde_json::Value>);
impl std::fmt::Debug for Patch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Patch([REDACTED])")
    }
}
#[derive(Default)]
pub struct State {
    pub selected: usize,
    pub edit: Option<String>,
    pub view: Option<DeciderView>,
    pub error: Option<String>,
    pub result: Option<Result<DeciderTestResult, String>>,
    pub patch: Patch,
    pub summary: Option<String>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SettingsState([REDACTED])")
    }
}
impl State {
    fn value(&self, i: usize) -> String {
        let Some(v) = &self.view else {
            return String::new();
        };
        match i {
            0 => v.endpoint.clone(),
            1 => v.model.clone(),
            2 => v.fallback_model.clone().unwrap_or_default(),
            3 => match v.on_unreachable {
                so_core::config::OnUnreachable::Error => "error",
                so_core::config::OnUnreachable::Heuristic => "heuristic",
            }
            .into(),
            4 => v.timeout.clone(),
            5 => v.min_confidence.to_string(),
            6 => v.min_margin.to_string(),
            _ => String::new(),
        }
    }
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if let Some(typed) = &mut self.edit {
            match key.code {
                KeyCode::Esc => self.edit = None,
                KeyCode::Enter => {
                    let value = self.edit.take().unwrap_or_default();
                    let i = self.selected;
                    if i >= 7 {
                        self.patch.0.insert(
                            "key".into(),
                            serde_json::json!({ ["env", "file", "paste"][i - 7]: value }),
                        );
                    } else if value != self.value(i) {
                        let v = if i == 5 || i == 6 {
                            value
                                .parse::<f64>()
                                .ok()
                                .and_then(serde_json::Number::from_f64)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::String(value))
                        } else {
                            serde_json::Value::String(value)
                        };
                        self.patch.0.insert(NAMES[i].into(), v);
                    } else {
                        self.patch.0.remove(NAMES[i]);
                    }
                }
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) => typed.push(c),
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(NAMES.len() - 1),
            KeyCode::Enter if self.view.is_some() => {
                if self.selected == 3 {
                    let current = self
                        .patch
                        .0
                        .get("on_unreachable")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| self.value(3));
                    let next = if current == "error" {
                        "heuristic"
                    } else {
                        "error"
                    };
                    if next == self.value(3) {
                        self.patch.0.remove("on_unreachable");
                    } else {
                        self.patch.0.insert("on_unreachable".into(), next.into());
                    }
                } else {
                    self.edit = Some(if self.selected >= 7 {
                        String::new()
                    } else {
                        self.patch
                            .0
                            .get(NAMES[self.selected])
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .unwrap_or_else(|| self.value(self.selected))
                    });
                }
            }
            KeyCode::Char('s') if !self.patch.0.is_empty() => {
                let patch = self.patch.clone();
                if self
                    .patch
                    .0
                    .get("key")
                    .is_some_and(|key| key.get("paste").is_some())
                {
                    self.patch.0.remove("key");
                }
                return Some(Effect::DeciderSet(patch));
            }
            KeyCode::Char('t') => return Some(Effect::DeciderTest),
            KeyCode::Char('r') => return Some(Effect::DeciderGet),
            _ => {}
        }
        None
    }
}
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.settings_tab;
    let Some(v) = &s.view else {
        f.render_widget(
            Paragraph::new(s.error.as_deref().unwrap_or("Loading Settings...")),
            area,
        );
        return;
    };
    let mut lines = vec![Line::raw(
        "Decider  ↑/↓ select  Enter edit  s save  t test  r reload",
    )];
    for (i, name) in NAMES.iter().enumerate() {
        let value = if i == s.selected && s.edit.is_some() {
            let e = s.edit.as_deref().unwrap_or_default();
            if i == 9 {
                "•".repeat(e.chars().count())
            } else {
                e.to_owned()
            }
        } else if i >= 7 {
            if s.patch
                .0
                .get("key")
                .is_some_and(|key| key.get(["env", "file", "paste"][i - 7]).is_some())
            {
                "(changed)".into()
            } else {
                String::new()
            }
        } else {
            s.patch
                .0
                .get(*name)
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_else(|| s.value(i))
        };
        let suggestion = match i {
            1 => format!("  (suggested: {})", v.suggestions.model),
            2 => format!(
                "  (suggested: {})",
                v.suggestions.fallback_model.as_deref().unwrap_or("none")
            ),
            _ => String::new(),
        };
        lines.push(Line::styled(
            format!(
                "{} {name:<16} {value}{suggestion}",
                if i == s.selected { ">" } else { " " }
            ),
            if i == s.selected {
                app.theme.selected()
            } else {
                app.theme.base()
            },
        ));
    }
    lines.push(Line::raw(format!("Running model: {}", v.effective.primary)));
    lines.push(Line::raw(format!(
        "Running fallback: {}",
        v.effective.fallback.as_deref().unwrap_or("none")
    )));
    lines.push(Line::raw(format!(
        "Key: {:?} {} {}",
        v.key.source,
        v.key.name.as_deref().unwrap_or(""),
        v.key.masked.as_deref().unwrap_or("")
    )));
    for o in &v.env_overrides {
        lines.push(Line::raw(format!(
            "{} wins over config.toml for {}",
            o.name, o.field
        )));
    }
    if v.restart_required {
        lines.push(Line::raw("Restart required to apply changes."));
    }
    if let Some(result) = &s.result {
        lines.push(Line::raw(match result {
            Err(e) => format!("Test: {e}"),
            Ok(r) if !r.ok => format!("Test: {}", r.error.as_deref().unwrap_or("failed")),
            Ok(r) => format!(
                "Test: complexity {} level {} confidence {} model {} latency {}ms",
                r.complexity.map(|v| v.to_string()).unwrap_or_default(),
                r.level.as_deref().unwrap_or("-"),
                r.confidence.map(|v| v.to_string()).unwrap_or_default(),
                r.model.as_deref().unwrap_or("-"),
                r.latency_ms
            ),
        }));
    }
    if let Some(summary) = &s.summary {
        lines.push(Line::raw(summary.clone()));
    }
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    #[test]
    fn editing_esc_cancels_and_paste_is_masked_in_debug() {
        let mut state = State {
            selected: 9,
            edit: Some(String::new()),
            ..Default::default()
        };
        for c in "secret".chars() {
            state.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(!format!("{state:?}").contains("secret"));
        state.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(state.edit.is_none());
    }
}
