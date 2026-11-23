use super::super::{
    app::{App, Effect},
    data::clean,
};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::Paragraph,
};
#[derive(Default)]
pub struct State {
    scroll: u16,
}
impl State {
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Char('r') => return Some(Effect::RefreshModels),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            _ => {}
        }
        None
    }
}
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = &app.snap else {
        f.render_widget(Paragraph::new("Loading Models..."), area);
        return;
    };
    let stamp = s
        .models_refreshed_at
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "never".into());
    let model_width = usize::from(area.width.saturating_sub(39)).max(1);
    let [heading, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!("Model catalog  |  last refresh: {stamp}"),
                app.theme.accent(),
            ),
            Line::raw(format!(
                "{:<10} {:<model_width$} {:<18} FREE",
                "HARNESS", "MODEL", "EFFORTS"
            )),
        ]),
        heading,
    );
    let mut lines = Vec::new();
    if let Some(error) = &s.catalog_error {
        lines.push(Line::raw(format!("Models unavailable: {}", clean(error))));
    } else if s.models.is_empty() {
        lines.push(Line::raw("No Models in the catalog."));
    } else {
        for m in &s.models {
            lines.push(Line::raw(format!(
                "{:<10} {:<model_width$} {:<18} {}",
                m.harness.to_string(),
                super::super::ui::truncate(&clean(&m.model), model_width, app.theme.ellipsis()),
                super::super::ui::truncate(&clean(&m.efforts.join(", ")), 18, app.theme.ellipsis()),
                if m.free { "yes" } else { "no" }
            )));
        }
    }
    let scroll = u16::min(
        app.models_tab.scroll,
        lines.len().saturating_sub(body.height as usize) as u16,
    );
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
}
