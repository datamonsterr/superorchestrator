//! Styles and symbols. Colour carries meaning only next to a symbol, so `NO_COLOR` and
//! 16-colour terminals lose nothing: bold, dim and reverse do the rest.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

#[derive(Debug, Clone)]
pub struct Theme {
    pub color: bool,
    pub unicode: bool,
}

/// Plain-ASCII frame for terminals without UTF-8.
const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

impl Theme {
    /// Reads `NO_COLOR`, `TERM` and the locale from the process environment.
    pub fn from_env() -> Self {
        Self::detect(|k| std::env::var(k).ok())
    }

    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Self {
        let no_color = env("NO_COLOR").is_some_and(|v| !v.is_empty());
        let term = env("TERM").unwrap_or_default();
        let dumb = term == "dumb";
        let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|k| env(k).filter(|v| !v.is_empty()))
            .is_some_and(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("utf-8") || v.contains("utf8")
            });
        Self {
            color: !no_color && !dumb,
            unicode: utf8 && term != "linux" && !dumb,
        }
    }

    #[cfg(test)]
    pub fn plain() -> Self {
        Self {
            color: false,
            unicode: true,
        }
    }

    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    pub fn base(&self) -> Style {
        Style::new()
    }
    pub fn muted(&self) -> Style {
        if self.color {
            Style::new().fg(Color::DarkGray)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }
    pub fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn accent(&self) -> Style {
        self.fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }
    pub fn ok(&self) -> Style {
        self.fg(Color::Green)
    }
    pub fn warn(&self) -> Style {
        self.fg(Color::Yellow)
    }
    pub fn bad(&self) -> Style {
        self.fg(Color::Red)
    }
    /// The selected row or tab: reverse video works in every colour mode.
    pub fn selected(&self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
    pub fn header(&self) -> Style {
        self.muted().add_modifier(Modifier::BOLD)
    }

    pub fn border_set(&self) -> border::Set<'static> {
        if self.unicode {
            border::ROUNDED
        } else {
            ASCII_BORDER
        }
    }

    pub fn tick(&self) -> &'static str {
        if self.unicode { "✓" } else { "+" }
    }
    pub fn cross(&self) -> &'static str {
        if self.unicode { "✗" } else { "x" }
    }
    pub fn bang(&self) -> &'static str {
        "!"
    }
    pub fn dot(&self) -> &'static str {
        if self.unicode { "●" } else { "*" }
    }
    pub fn circle(&self) -> &'static str {
        if self.unicode { "○" } else { "o" }
    }
    pub fn pointer(&self) -> &'static str {
        if self.unicode { "▸" } else { ">" }
    }
    pub fn arrow(&self) -> &'static str {
        if self.unicode { "→" } else { "->" }
    }
    pub fn arrow_down(&self) -> &'static str {
        if self.unicode { "↓" } else { "v" }
    }
    pub fn ellipsis(&self) -> &'static str {
        if self.unicode { "…" } else { "..." }
    }
    pub fn spinner(&self, tick: u64) -> &'static str {
        const U: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        const A: [&str; 4] = ["|", "/", "-", "\\"];
        if self.unicode {
            U[(tick % 8) as usize]
        } else {
            A[(tick % 4) as usize]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn no_color_and_dumb_terminals_drop_colour() {
        assert!(Theme::detect(env(&[("LANG", "en_US.UTF-8")])).color);
        assert!(!Theme::detect(env(&[("NO_COLOR", "1")])).color);
        assert!(Theme::detect(env(&[("NO_COLOR", "")])).color);
        assert!(!Theme::detect(env(&[("TERM", "dumb")])).color);
    }

    #[test]
    fn unicode_needs_a_utf8_locale_and_a_capable_terminal() {
        assert!(Theme::detect(env(&[("LANG", "en_US.UTF-8")])).unicode);
        assert!(!Theme::detect(env(&[("LANG", "C")])).unicode);
        assert!(!Theme::detect(env(&[])).unicode);
        assert!(!Theme::detect(env(&[("LANG", "en_US.UTF-8"), ("TERM", "linux")])).unicode);
        assert!(Theme::detect(env(&[("LC_ALL", "C.utf8"), ("LANG", "C")])).unicode);
    }

    #[test]
    fn no_color_styles_carry_no_colour() {
        let t = Theme {
            color: false,
            unicode: true,
        };
        for s in [t.ok(), t.warn(), t.bad(), t.accent(), t.muted()] {
            assert!(s.fg.is_none() && s.bg.is_none());
        }
    }
}
