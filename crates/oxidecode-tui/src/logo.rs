//! The exact OpenCode wordmark, rendered with Ratatui spans.
//!
//! The source art uses `_`, `^`, `~` and `,` as shading placeholders; they are
//! translated the same way OpenCode's `Logo` component does.

use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub const LOGO_LEFT: [&str; 4] = [
    "                   ",
    "█▀▀█ █▀▀█ █▀▀█ █▀▀▄",
    "█__█ █__█ █^^^ █__█",
    "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀",
];

pub const LOGO_RIGHT: [&str; 4] = [
    "             ▄     ",
    "█▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█___ █__█ █__█ █^^^",
    "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

pub const GO_LEFT: [&str; 4] = ["    ", "█▀▀▀", "█_^█", "▀▀▀▀"];
pub const GO_RIGHT: [&str; 4] = ["    ", "█▀▀█", "█__█", "▀▀▀▀"];

fn render_run(line: &str, fg: ratatui::style::Color, shadow: ratatui::style::Color, bold: bool, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.chars().count());
    for ch in line.chars() {
        let (text, style) = match ch {
            '_' => (
                " ".to_string(),
                Style::default().bg(shadow),
            ),
            '^' => (
                "▀".to_string(),
                Style::default().fg(fg).bg(shadow),
            ),
            '~' => (
                "▀".to_string(),
                Style::default().fg(shadow),
            ),
            ',' => (
                "▄".to_string(),
                Style::default().fg(shadow),
            ),
            other => {
                let mut s = Style::default().fg(fg);
                if bold {
                    s = s.add_modifier(Modifier::BOLD);
                }
                (other.to_string(), s)
            }
        };
        spans.push(Span::styled(text, style));
    }
    let _ = theme;
    spans
}

fn left_spans(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    render_run(line, theme.muted, theme.shadow(theme.muted), false, theme)
}

fn right_spans(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    render_run(line, theme.text, theme.shadow(theme.text), true, theme)
}

/// Builds the logo for a terminal of the given width, following OpenCode's
/// breakpoints (stacked below 44 columns, compact mark below 22).
pub fn lines(width: u16, height: u16, theme: &Theme) -> Vec<Line<'static>> {
    if height < 12 {
        return Vec::new();
    }
    if width < 22 {
        return GO_RIGHT
            .iter()
            .skip(1)
            .map(|line| Line::from(render_run(line, theme.text, theme.shadow(theme.text), true, theme)))
            .collect();
    }
    if width < 44 {
        let mut out = Vec::new();
        for line in LOGO_LEFT.iter().skip(1) {
            out.push(Line::from(left_spans(line, theme)));
        }
        for line in LOGO_RIGHT.iter() {
            out.push(Line::from(right_spans(line, theme)));
        }
        return out;
    }
    let mut out = Vec::new();
    for index in 0..4 {
        let mut spans = left_spans(LOGO_LEFT[index], theme);
        spans.push(Span::raw(" "));
        spans.extend(right_spans(LOGO_RIGHT[index], theme));
        out.push(Line::from(spans));
    }
    out
}

/// The visible width of the wide logo.
pub const WIDTH_WIDE: u16 = 39;
pub const WIDTH_COMPACT: u16 = 19;
pub const WIDTH_GO: u16 = 4;
