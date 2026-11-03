use super::super::{
    app::{App, Effect},
    data::{SessionItem, clean},
    ui,
};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Margin, Rect},
    widgets::{Cell, Paragraph, Row, Table, TableState},
};

#[derive(Default)]
pub struct State {
    pub sel: usize,
    pub run_sel: usize,
    pub open: Option<String>,
    pub table: TableState,
    pub runs_table: TableState,
}
impl State {
    pub fn on_key(&mut self, key: KeyEvent, sessions: &[SessionItem]) -> Option<Effect> {
        if let Some(id) = &self.open {
            let runs = &sessions.iter().find(|s| &s.id == id)?.runs;
            if let Some(sel) = App::movement(key.code, self.run_sel, runs.len()) {
                self.run_sel = sel;
            } else if key.code == KeyCode::Enter {
                return Some(Effect::Detail {
                    id: runs.get(self.run_sel)?.id.clone(),
                    diff: true,
                });
            }
        } else if let Some(sel) = App::movement(key.code, self.sel, sessions.len()) {
            self.sel = sel;
        } else if key.code == KeyCode::Enter {
            self.open = Some(sessions.get(self.sel)?.id.clone());
            self.run_sel = 0;
        }
        None
    }
}
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(s) = &app.snap else {
        f.render_widget(Paragraph::new("Loading Sessions..."), area);
        return;
    };
    if let Some(id) = &app.sessions_tab.open {
        if let Some(session) = s.sessions.iter().find(|s| &s.id == id) {
            if session.runs.is_empty() {
                f.render_widget(Paragraph::new("No Runs in this Session."), area);
            } else {
                ui::draw_run_table(
                    f,
                    &session.runs,
                    app.now,
                    &app.theme,
                    app.sessions_tab.run_sel,
                    &mut app.sessions_tab.runs_table,
                    area,
                );
            }
        }
        return;
    }
    if let Some(error) = &s.sessions_error {
        f.render_widget(
            Paragraph::new(format!("Sessions unavailable: {}", clean(error))),
            area,
        );
        return;
    }
    if s.sessions.is_empty() {
        f.render_widget(Paragraph::new("No Sessions yet."), area);
        return;
    }
    let rows = s.sessions.iter().map(|s| {
        Row::new(vec![
            Cell::new(s.id.chars().take(8).collect::<String>()),
            Cell::new(ui::age(app.now, s.started_at)),
            Cell::new(s.run_count.to_string()),
            Cell::new(
                s.runs
                    .iter()
                    .filter(|r| r.state.is_active())
                    .count()
                    .to_string(),
            ),
            Cell::new(clean(&s.title)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new(["SESSION", "STARTED", "RUNS", "ACTIVE", "TITLE / CWD"]).style(app.theme.header()),
    )
    .column_spacing(1)
    .highlight_symbol(format!("{} ", app.theme.pointer()))
    .row_highlight_style(app.theme.selected());
    app.sessions_tab.table.select(Some(app.sessions_tab.sel));
    f.render_stateful_widget(
        table,
        area.inner(Margin::new(1, 0)),
        &mut app.sessions_tab.table,
    );
}
