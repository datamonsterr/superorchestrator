use super::super::app::{App, Effect};
use ratatui::{Frame, crossterm::event::KeyEvent, layout::Rect, widgets::Paragraph};
#[derive(Default)]
pub struct State;
impl State {
    pub fn on_key(&mut self, _: KeyEvent) -> Option<Effect> {
        None
    }
}
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new("Sessions: not loaded").style(app.theme.muted()),
        area,
    );
}
