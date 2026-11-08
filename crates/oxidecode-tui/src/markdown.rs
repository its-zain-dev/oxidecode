//! Markdown to Ratatui lines with OpenCode's theme colours.
//!
//! The renderer is intentionally allocation-light: it walks pulldown-cmark
//! events into styled runs, wraps those runs to the terminal width and only
//! then converts them to `Line`s. During streaming the caller can use the
//! cheaper `plain` renderer and switch to Markdown once the message ends.

use crate::theme::Theme;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone)]
struct StyledChar {
    ch: char,
    style: Style,
}

#[derive(Debug, Clone, Default)]
struct RichLine {
    chars: Vec<StyledChar>,
}

impl RichLine {
    fn push(&mut self, text: &str, style: Style) {
        for ch in text.chars() {
            self.chars.push(StyledChar { ch, style });
        }
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    fn width(&self) -> usize {
        self.chars
            .iter()
            .map(|item| item.ch.width().unwrap_or(1).max(1))
            .sum()
    }

    fn trim_end(&mut self) {
        while self
            .chars
            .last()
            .map(|item| item.ch.is_whitespace())
            .unwrap_or(false)
        {
            self.chars.pop();
        }
    }
}

#[derive(Default, Clone, Copy)]
struct StyleState {
    bold: bool,
    italic: bool,
    strike: bool,
    link: bool,
    heading: Option<u8>,
    quote: usize,
    in_code: bool,
}

impl StyleState {
    fn style(&self, theme: &Theme) -> Style {
        let mut style = Style::default().fg(theme.text);
        if self.in_code {
            style = style.fg(theme.markdown_code).bg(theme.background_raised);
        }
        if let Some(level) = self.heading {
            style = style
                .fg(theme.markdown_heading)
                .add_modifier(Modifier::BOLD);
            if level >= 3 {
                style = style.add_modifier(Modifier::DIM);
            }
        }
        if self.quote > 0 {
            style = style.fg(theme.markdown_quote);
        }
        if self.link {
            style = style.fg(theme.markdown_link_text).add_modifier(Modifier::UNDERLINED);
        }
        if self.bold {
            style = style
                .fg(theme.markdown_strong)
                .add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.fg(theme.markdown_emphasis).add_modifier(Modifier::ITALIC);
        }
        if self.strike {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        style
    }
}

#[derive(Default)]
struct ListState {
    start: Option<u64>,
    index: u64,
}

pub fn render(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut out: Vec<RichLine> = Vec::new();
    let mut current = RichLine::default();
    let mut state = StyleState::default();
    let mut lists: Vec<ListState> = Vec::new();
    let mut code: Option<(Option<String>, String)> = None;
    let mut heading: Option<u8> = None;

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    if state.quote > 0 {
                        current.push("│ ", Style::default().fg(theme.markdown_quote));
                    }
                }
                Tag::Heading { level, .. } => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    state.heading = Some(level as u8);
                    heading = Some(level as u8);
                }
                Tag::BlockQuote(_) => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    state.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => {
                            let language = info.split_whitespace().next().unwrap_or("");
                            if language.is_empty() {
                                None
                            } else {
                                Some(language.to_string())
                            }
                        }
                        CodeBlockKind::Indented => None,
                    };
                    code = Some((language, String::new()));
                }
                Tag::List(start) => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    lists.push(ListState {
                        start,
                        index: start.unwrap_or(1),
                    });
                }
                Tag::Item => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    let depth = lists.len().saturating_sub(1);
                    current.push(&"  ".repeat(depth), Style::default().fg(theme.muted));
                    if let Some(list) = lists.last_mut() {
                        if list.start.is_some() {
                            current.push(
                                &format!("{}. ", list.index),
                                Style::default().fg(theme.markdown_enum),
                            );
                            list.index += 1;
                        } else {
                            current.push("• ", Style::default().fg(theme.markdown_list));
                        }
                    }
                }
                Tag::Emphasis => state.italic = true,
                Tag::Strong => state.bold = true,
                Tag::Strikethrough => state.strike = true,
                Tag::Link { dest_url, .. } => {
                    state.link = true;
                    current.push("", Style::default());
                    let _ = dest_url;
                }
                Tag::TableHead => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    state.bold = true;
                }
                Tag::TableRow => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                }
                Tag::TableCell => {
                    if !current.is_empty() {
                        current.push(" │ ", Style::default().fg(theme.muted));
                    }
                }
                Tag::Image { .. } => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    flush(&mut out, &mut current);
                    paragraph_gap(&mut out, &mut current);
                }
                TagEnd::Heading(_) => {
                    state.heading = None;
                    flush(&mut out, &mut current);
                    let _ = heading.take();
                    paragraph_gap(&mut out, &mut current);
                }
                TagEnd::BlockQuote(_) => {
                    state.quote = state.quote.saturating_sub(1);
                    flush(&mut out, &mut current);
                    paragraph_gap(&mut out, &mut current);
                }
                TagEnd::CodeBlock => {
                    if let Some((language, source)) = code.take() {
                        let lines = highlight_code(&source, language.as_deref(), theme);
                        for line in lines {
                            out.push(line);
                        }
                        out.push(RichLine::default());
                    }
                }
                TagEnd::List(_) => {
                    lists.pop();
                    flush(&mut out, &mut current);
                    if lists.is_empty() {
                        paragraph_gap(&mut out, &mut current);
                    }
                }
                TagEnd::Item => {
                    flush(&mut out, &mut current);
                }
                TagEnd::Emphasis => state.italic = false,
                TagEnd::Strong => state.bold = false,
                TagEnd::Strikethrough => state.strike = false,
                TagEnd::Link => state.link = false,
                TagEnd::TableHead => {
                    state.bold = false;
                    flush(&mut out, &mut current);
                }
                TagEnd::TableRow => {
                    flush(&mut out, &mut current);
                }
                TagEnd::TableCell => {}
                TagEnd::Image => {}
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, source)) = code.as_mut() {
                    source.push_str(&text);
                } else {
                    current.push(&text, state.style(theme));
                }
            }
            Event::Code(text) => {
                current.push(&text, state.style(theme).fg(theme.markdown_code).bg(theme.background_raised));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if code.is_none() {
                    current.push(html.trim(), Style::default().fg(theme.muted));
                }
            }
            Event::SoftBreak => current.push(" ", Style::default()),
            Event::HardBreak => {
                flush(&mut out, &mut current);
                if state.quote > 0 {
                    current.push("│ ", Style::default().fg(theme.markdown_quote));
                }
            }
            Event::Rule => {
                flush(&mut out, &mut current);
                let rule = RichLine {
                    chars: std::iter::repeat_n(
                        StyledChar {
                            ch: '─',
                            style: Style::default().fg(theme.markdown_rule),
                        },
                        width.min(48),
                    )
                    .collect(),
                };
                out.push(rule);
                paragraph_gap(&mut out, &mut current);
            }
            Event::TaskListMarker(done) => {
                current.push(
                    if done { "[x] " } else { "[ ] " },
                    Style::default().fg(if done { theme.success } else { theme.muted }),
                );
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                current.push(&math, Style::default().fg(theme.markdown_code));
            }
            Event::FootnoteReference(name) => {
                current.push(&format!("[{name}]"), Style::default().fg(theme.markdown_link));
            }
        }
    }
    flush(&mut out, &mut current);

    let mut lines = Vec::new();
    for line in out {
        let trimmed = line.chars.iter().any(|item| !item.ch.is_whitespace());
        if !trimmed {
            if lines
                .last()
                .map(|line: &Line<'static>| line.spans.iter().all(|span| span.content.trim().is_empty()))
                .unwrap_or(true)
            {
                continue;
            }
            lines.push(Line::from(""));
            continue;
        }
        wrap_rich(line, width, true, &mut lines);
    }
    while lines
        .last()
        .map(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        .unwrap_or(false)
    {
        lines.pop();
    }
    lines
}

fn flush(out: &mut Vec<RichLine>, current: &mut RichLine) {
    if !current.is_empty() {
        out.push(std::mem::take(current));
    }
}

fn paragraph_gap(out: &mut Vec<RichLine>, current: &mut RichLine) {
    if !out.is_empty() && current.is_empty() {
        if !out
            .last()
            .map(|line| line.chars.is_empty())
            .unwrap_or(true)
        {
            out.push(RichLine::default());
        }
    }
}

fn wrap_rich(line: RichLine, width: usize, word_wrap: bool, out: &mut Vec<Line<'static>>) {
    let mut start = 0usize;
    let total = line.chars.len();
    while start < total {
        let mut end = start;
        let mut cells = 0usize;
        let mut last_space: Option<usize> = None;
        while end < total {
            let ch = line.chars[end].ch;
            let ch_width = ch.width().unwrap_or(1).max(1);
            if cells + ch_width > width && end > start {
                break;
            }
            if ch == ' ' {
                last_space = Some(end);
            }
            cells += ch_width;
            end += 1;
        }
        if word_wrap && end < total {
            if let Some(space) = last_space {
                if space > start && space + 1 < end {
                    end = space;
                }
            }
        }
        let mut slice = RichLine {
            chars: line.chars[start..end].to_vec(),
        };
        slice.trim_end();
        out.push(to_line(slice));
        start = end;
        while start < total && line.chars[start].ch == ' ' && word_wrap {
            start += 1;
        }
    }
    if total == 0 {
        out.push(Line::from(""));
    }
}

fn to_line(line: RichLine) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current = String::new();
    let mut style = Style::default();
    for item in line.chars {
        if current.is_empty() {
            style = item.style;
            current.push(item.ch);
        } else if item.style == style {
            current.push(item.ch);
        } else {
            spans.push(Span::styled(std::mem::take(&mut current), style));
            style = item.style;
            current.push(item.ch);
        }
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, style));
    }
    Line::from(spans)
}

/// Cheap renderer used while a message is still streaming.
pub fn plain(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(theme.muted),
            )));
            continue;
        }
        let style = if in_code {
            Style::default().fg(theme.markdown_code).bg(theme.background_raised)
        } else {
            Style::default().fg(theme.text)
        };
        let line = RichLine {
            chars: raw
                .chars()
                .map(|ch| StyledChar { ch, style })
                .collect(),
        };
        wrap_rich(line, width, true, &mut out);
    }
    out
}

fn highlight_code(source: &str, language: Option<&str>, theme: &Theme) -> Vec<RichLine> {
    let base = Style::default()
        .fg(theme.markdown_code)
        .bg(theme.background_raised);
    let mut lines = Vec::new();
    for raw in source.lines() {
        let mut line = RichLine::default();
        let trimmed = raw.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("--")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            line.push(raw, Style::default().fg(theme.syntax_comment).bg(theme.background_raised));
            lines.push(line);
            continue;
        }
        highlight_line(raw, language, theme, base, &mut line);
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(RichLine::default());
    }
    lines
}

fn highlight_line(
    raw: &str,
    language: Option<&str>,
    theme: &Theme,
    base: Style,
    line: &mut RichLine,
) {
    let keywords: &[&str] = match language.unwrap_or("") {
        "rust" | "rs" => &[
            "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "match",
            "if", "else", "for", "while", "loop", "return", "async", "await", "move", "where",
            "const", "static", "type", "self", "Self", "crate", "super", "unsafe", "dyn", "ref",
        ],
        "js" | "javascript" | "ts" | "typescript" | "tsx" | "jsx" => &[
            "const", "let", "var", "function", "return", "if", "else", "for", "while", "class",
            "extends", "new", "async", "await", "import", "export", "from", "default", "try",
            "catch", "throw", "interface", "type", "enum", "implements", "public", "private",
        ],
        "python" | "py" => &[
            "def", "class", "return", "if", "elif", "else", "for", "while", "import", "from",
            "as", "try", "except", "finally", "with", "lambda", "yield", "async", "await", "pass",
            "raise", "in", "not", "and", "or", "is", "None", "True", "False",
        ],
        "c" | "h" | "cpp" | "cc" => &[
            "int", "char", "void", "struct", "enum", "union", "static", "const", "return", "if",
            "else", "for", "while", "switch", "case", "break", "continue", "typedef", "sizeof",
            "unsigned", "signed", "long", "short", "float", "double", "extern",
        ],
        "json" => &["true", "false", "null"],
        _ => &[
            "fn", "let", "const", "function", "return", "if", "else", "for", "while", "class",
            "import", "export", "true", "false", "null",
        ],
    };
    let chars: Vec<char> = raw.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '"' || ch == '\'' || ch == '`' {
            let quote = ch;
            let mut end = index + 1;
            while end < chars.len() {
                if chars[end] == '\\' {
                    end += 2;
                    continue;
                }
                if chars[end] == quote {
                    end += 1;
                    break;
                }
                end += 1;
            }
            let text: String = chars[index..end.min(chars.len())].iter().collect();
            line.push(&text, base.fg(theme.syntax_string));
            index = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '.') {
                end += 1;
            }
            let text: String = chars[index..end].iter().collect();
            line.push(&text, base.fg(theme.syntax_number));
            index = end;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let word: String = chars[index..end].iter().collect();
            let style = if keywords.contains(&word.as_str()) {
                base.fg(theme.syntax_keyword)
            } else if chars.get(end) == Some(&'(') {
                base.fg(theme.syntax_function)
            } else {
                base
            };
            line.push(&word, style);
            index = end;
            continue;
        }
        let style = if "=+-*/<>!&|".contains(ch) {
            base.fg(theme.syntax_operator)
        } else {
            base
        };
        line.push(&ch.to_string(), style);
        index += 1;
    }
}

/// Wraps plain text into owned lines with a base style.
pub fn wrap_text(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = RichLine {
            chars: raw.chars().map(|ch| StyledChar { ch, style }).collect(),
        };
        wrap_rich(line, width.max(4), true, &mut out);
    }
    out
}

/// Visible width of a string.
pub fn width_of(text: &str) -> usize {
    text.chars()
        .map(|ch| ch.width().unwrap_or(1).max(1))
        .sum()
}

pub fn truncate(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut cells = 0usize;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(1).max(1);
        if cells + w > width - 1 {
            break;
        }
        out.push(ch);
        cells += w;
    }
    out.push('…');
    out
}

pub fn slice_by_width(text: &str, range: Range<usize>) -> String {
    let mut out = String::new();
    let mut cells = 0usize;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(1).max(1);
        if cells >= range.end {
            break;
        }
        if cells >= range.start {
            out.push(ch);
        }
        cells += w;
    }
    out
}
