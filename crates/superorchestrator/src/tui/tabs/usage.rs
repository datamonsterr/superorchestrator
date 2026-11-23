use super::super::{
    app::{App, Effect},
    data::clean,
};
use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Paragraph, Sparkline},
};

#[derive(Default)]
pub struct State {
    scroll: u16,
}
impl State {
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            _ => {}
        }
        None
    }
}

fn relative(now: DateTime<Utc>, until: DateTime<Utc>) -> String {
    let mins = (until - now).num_minutes().max(0);
    if mins >= 1440 {
        format!("{}d {}h", mins / 1440, (mins % 1440) / 60)
    } else if mins >= 60 {
        format!("{}h {}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}
fn window(
    label: &str,
    pct: Option<f64>,
    reset: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> String {
    let bar = match pct {
        Some(p) if p.is_finite() => {
            let p = p.clamp(0.0, 100.0);
            let filled = (p / 10.0).round() as usize;
            format!(
                "[{}{}] {p:.0}%",
                "█".repeat(filled),
                "░".repeat(10 - filled)
            )
        }
        _ => "[░░░░░░░░░░] unknown".into(),
    };
    let reset = reset
        .map(|t| format!("  resets in {}", relative(now, t)))
        .unwrap_or_default();
    format!("  {label:<6} {bar}{reset}")
}
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = &app.snap else {
        f.render_widget(Paragraph::new("Loading usage…"), area);
        return;
    };
    let [accounts, graph] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).areas(area);
    let mut lines = vec![Line::styled(
        "Account and pool headroom",
        app.theme.accent(),
    )];
    if let Some(error) = &s.accounts_error {
        lines.push(Line::raw(format!("Accounts unavailable: {}", clean(error))));
    } else if s.accounts.is_empty() {
        lines.push(Line::raw("No accounts configured."));
    }
    for a in &s.accounts {
        if a.pools.is_empty() {
            lines.push(Line::raw(format!("{}: no pools", clean(&a.name))));
        }
        for pool in &a.pools {
            let h = pool.headroom.as_ref();
            let status = h.map_or("unknown", |h| h.status.as_str());
            let cooldown = pool
                .cooldown_until
                .filter(|t| *t > app.now)
                .map(|t| {
                    format!(
                        "  cooldown until {} (in {})",
                        t.format("%Y-%m-%d %H:%M UTC"),
                        relative(app.now, t)
                    )
                })
                .unwrap_or_default();
            lines.push(Line::raw(format!(
                "{} / {}: {status}{cooldown}",
                clean(&a.name),
                clean(&pool.pool)
            )));
            lines.push(Line::raw(window(
                "5-hour",
                h.and_then(|h| h.five_hour_remaining_pct),
                h.and_then(|h| h.five_hour_resets_at),
                app.now,
            )));
            lines.push(Line::raw(window(
                "weekly",
                h.and_then(|h| h.weekly_remaining_pct),
                h.and_then(|h| h.weekly_resets_at),
                app.now,
            )));
        }
    }
    let max_scroll = lines.len().saturating_sub(usize::from(accounts.height));
    f.render_widget(
        Paragraph::new(lines).scroll((app.usage_tab.scroll.min(max_scroll as u16), 0)),
        accounts,
    );
    let [title, chart] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(graph);
    if let Some(error) = &s.usage_error {
        f.render_widget(
            Paragraph::new(format!("Usage unavailable: {}", clean(error))),
            title,
        );
    } else if s.daily_usage.is_empty() {
        f.render_widget(Paragraph::new("No daily usage yet."), title);
    } else {
        f.render_widget(
            Paragraph::new("Daily usage (tokens, last 14 days)").style(app.theme.accent()),
            title,
        );
        let today = app.now.date_naive();
        let values: Vec<u64> = (0..14)
            .rev()
            .map(|d| {
                let day = (today - chrono::Duration::days(d)).to_string();
                s.daily_usage
                    .iter()
                    .find(|r| r.day == day)
                    .map_or(0, |r| r.input_tokens.saturating_add(r.output_tokens))
            })
            .collect();
        f.render_widget(
            Sparkline::default().data(&values).style(app.theme.accent()),
            chart,
        );
    }
}
