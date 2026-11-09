//! Ratatui renderer for the OxideCode terminal interface.
//!
//! The layout mirrors OpenCode's V2 TUI: a centred wordmark and prompt on the
//! home screen, a 64-column transcript with a 42-column elevated sidebar in
//! sessions, and a single-line status bar. Colours come from `theme.rs`.

use crate::app::{App, Dialog, DialogItem, ListDialog, Screen, SessionView};
use crate::input::Input;
use crate::markdown::{truncate, width_of};
use crate::theme::Theme;
use oxidecode_core::session::{
    AssistantMessage, AssistantPart, Message, SessionInfo, ShellMessage, ToolPart, ToolState,
    UserMessage,
};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;
use std::path::Path;

pub const VERSION: &str = oxidecode_core::API_VERSION;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SIDEBAR_WIDTH: u16 = 42;
const CONTENT_WIDTH: u16 = 64;

pub fn draw(frame: &mut Frame, app: &mut App) -> Option<(u16, u16)> {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let theme = app.theme.clone();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let body = chunks[0];
    let status = chunks[1];

    let mut cursor = match app.screen {
        Screen::Home => draw_home(frame, app, body, &theme),
        Screen::Session => draw_session(frame, app, body, &theme),
    };

    draw_status(frame, app, status, &theme);

    if app.screen == Screen::Session {
        draw_permission(frame, app, body, &theme);
    }

    if app.dialog.is_some() {
        cursor = None;
        draw_dialog(frame, app, body, &theme);
    }

    if let Some(toast) = &app.toast {
        draw_toast(frame, body, &theme, &toast.message, toast.error);
    }

    cursor
}

fn draw_home(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) -> Option<(u16, u16)> {
    let width = area.width.saturating_sub(4).min(75).max(16);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let input_width = width.saturating_sub(3) as usize;
    let rows = app.home.input.visual_lines(input_width).len().clamp(1, 10);
    let prompt_height = rows as u16 + 1;

    let show_logo = area.height >= 14;
    let logo_height = if show_logo { 4 } else { 0 };
    let logo_gap = if show_logo { 1 } else { 0 };
    let total = logo_height + logo_gap + prompt_height;
    let top = area.y + area.height.saturating_sub(total) / 2;

    if show_logo {
        let lines = crate::logo::lines(width, area.height, theme);
        let logo_area = Rect::new(x, top, width, lines.len() as u16);
        frame.render_widget(Paragraph::new(lines), logo_area);
    }

    let prompt_y = top + logo_height + logo_gap;
    let prompt_area = Rect::new(x, prompt_y, width, prompt_height);
    let placeholder = if app.home.submitting {
        "Creating session…"
    } else {
        "Ask anything…"
    };
    let cursor = draw_prompt(
        frame,
        theme,
        prompt_area,
        &app.home.input,
        placeholder,
        !app.home.submitting,
        if app.home.submitting {
            "starting a new session"
        } else {
            "enter send · shift+enter newline · tab agents · ctrl+p commands"
        },
    );
    if !app.home.submitting {
        cursor
    } else {
        None
    }
}

fn draw_session(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) -> Option<(u16, u16)> {
    let Some(session) = app.session.as_ref() else {
        return None;
    };

    let sidebar = app.visible_sidebar(area.width);
    let (main, side) = if sidebar && area.width >= SIDEBAR_WIDTH + 48 {
        let split = Layout::horizontal([
            Constraint::Min(48),
            Constraint::Length(SIDEBAR_WIDTH),
        ])
        .split(area);
        (split[0], Some(split[1]))
    } else {
        (area, None)
    };

    let content_width = main.width.saturating_sub(4).min(CONTENT_WIDTH).max(20);
    let content_x = main.x + (main.width.saturating_sub(content_width)) / 2;
    let input_width = content_width.saturating_sub(3) as usize;
    let rows = session.input.visual_lines(input_width).len().clamp(1, 8);
    let prompt_height = rows as u16 + 1;
    let prompt_y = main.y + main.height.saturating_sub(prompt_height);
    let transcript_height = prompt_y.saturating_sub(main.y).saturating_sub(1).max(1);

    let transcript = Rect::new(
        content_x,
        main.y + 1,
        content_width,
        transcript_height.saturating_sub(1).max(1),
    );

    let lines = transcript_lines(app, session, content_width as usize, theme);
    let total = lines.len() as u16;
    let viewport = transcript.height;
    let bottom = total.saturating_sub(viewport);
    let scroll = (bottom.saturating_sub(session.scroll as u16)) as u16;
    let paragraph = Paragraph::new(Text::from(lines)).scroll((scroll, 0));
    frame.render_widget(paragraph, transcript);

    let prompt_area = Rect::new(content_x, prompt_y, content_width, prompt_height);
    let cursor = draw_prompt(
        frame,
        theme,
        prompt_area,
        &session.input,
        if session.running {
            "Queue a follow-up…"
        } else {
            "Ask anything…"
        },
        true,
        if session.running {
            "esc interrupt · enter queue"
        } else {
            "enter send · shift+enter newline · tab agents · ctrl+p commands"
        },
    );

    if let Some(side) = side {
        draw_sidebar(frame, app, session, side, theme);
    }

    cursor
}

fn draw_prompt(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    input: &Input,
    placeholder: &str,
    focused: bool,
    hint: &str,
) -> Option<(u16, u16)> {
    if area.height < 2 || area.width < 4 {
        return None;
    }
    let border_color = if focused {
        theme.interactive
    } else {
        theme.border
    };
    let input_width = area.width.saturating_sub(3) as usize;
    let lines = input.visual_lines(input_width);
    let (cursor_row, cursor_col) = input.visual_position(&lines, input_width);
    let visible = area.height.saturating_sub(1) as usize;
    let start = if cursor_row >= visible {
        cursor_row + 1 - visible
    } else {
        0
    };

    for row in 0..visible {
        let y = area.y + row as u16;
        let index = start + row;
        let border = Span::styled(
            if row == 0 { "┃" } else { "│" },
            Style::default().fg(border_color),
        );
        let content = if let Some(line) = lines.get(index) {
            if input.is_empty() && index == 0 {
                Span::styled(placeholder.to_string(), Style::default().fg(theme.muted))
            } else {
                Span::styled(line.text.clone(), Style::default().fg(theme.text))
            }
        } else {
            Span::raw("")
        };
        let line = Line::from(vec![border, Span::raw(" "), content]);
        let rect = Rect::new(area.x, y, area.width, 1);
        frame.render_widget(Paragraph::new(line), rect);
    }

    let hint_y = area.y + area.height - 1;
    let hint_line = Line::from(vec![
        Span::styled("╹", Style::default().fg(border_color)),
        Span::raw(" "),
        Span::styled(hint.to_string(), Style::default().fg(theme.muted)),
    ]);
    frame.render_widget(
        Paragraph::new(hint_line),
        Rect::new(area.x, hint_y, area.width, 1),
    );

    let cursor_y = area.y + (cursor_row.saturating_sub(start)) as u16;
    Some((area.x + 2 + cursor_col as u16, cursor_y))
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background_raised)),
        area,
    );
    if area.width < 8 {
        return;
    }
    let inner = Rect::new(area.x + 2, area.y, area.width.saturating_sub(4), 1);

    let model = app
        .status_model()
        .map(|model| model.display())
        .unwrap_or_else(|| "no model".to_string());
    let agent = app.status_agent();
    let mut left: Vec<Span> = vec![
        Span::styled(
            agent,
            Style::default()
                .fg(theme.interactive)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(theme.faint)),
        Span::styled(model, Style::default().fg(theme.muted)),
    ];
    if app
        .session
        .as_ref()
        .map(|session| session.running)
        .unwrap_or(false)
    {
        left.push(Span::styled("  ", Style::default()));
        left.push(Span::styled(
            SPINNER[app.spinner % SPINNER.len()],
            Style::default().fg(theme.interactive),
        ));
    }

    let right = match app.screen {
        Screen::Home => format!("{} ", shorten_dir(&app.directory)),
        Screen::Session => {
            let used = app.context_tokens();
            let limit = app
                .session
                .as_ref()
                .map(|session| session.context_limit)
                .unwrap_or(200_000);
            format!("{} / {} ", tokens(used), tokens(limit))
        }
    };
    let dot = if app.connected {
        Span::styled("● ", Style::default().fg(theme.success))
    } else {
        Span::styled("● ", Style::default().fg(theme.error))
    };
    let right_width = width_of(&right) + 2;

    let mut spans = left;
    let left_width: usize = spans.iter().map(|span| width_of(&span.content)).sum();
    let inner_width = inner.width as usize;
    let padding = inner_width.saturating_sub(left_width + right_width);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(dot);
    spans.push(Span::styled(right, Style::default().fg(theme.muted)));

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, inner);
}

fn draw_sidebar(
    frame: &mut Frame,
    app: &App,
    session: &SessionView,
    area: Rect,
    theme: &Theme,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background_raised)),
        area,
    );
    if area.width < 8 || area.height < 4 {
        return;
    }
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let width = inner.width as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();

    let title_style = Style::default()
        .fg(theme.text)
        .add_modifier(Modifier::BOLD);
    lines.extend(crate::markdown::wrap_text(
        if session.title.is_empty() {
            "Untitled session"
        } else {
            &session.title
        },
        width,
        title_style,
    ));
    lines.push(Line::from(""));

    let section = |lines: &mut Vec<Line<'static>>, label: &str, value: &str| {
        lines.push(Line::from(Span::styled(
            label.to_string(),
            Style::default()
                .fg(theme.faint)
                .add_modifier(Modifier::BOLD),
        )));
        lines.extend(crate::markdown::wrap_text(
            value,
            width,
            Style::default().fg(theme.text),
        ));
        lines.push(Line::from(""));
    };

    let model = session
        .model
        .as_ref()
        .map(|model| model.display())
        .unwrap_or_else(|| "no model".to_string());
    section(&mut lines, "MODEL", &model);
    section(&mut lines, "AGENT", &session.agent);
    section(&mut lines, "DIRECTORY", &shorten_dir(&app.directory));

    let used = app.context_tokens();
    let limit = session.context_limit.max(1);
    let ratio = (used as f64 / limit as f64).clamp(0.0, 1.0);
    let filled = (ratio * 24.0).round() as usize;
    let mut bar = String::new();
    for index in 0..24 {
        bar.push(if index < filled { '█' } else { '░' });
    }
    lines.push(Line::from(Span::styled(
        "CONTEXT",
        Style::default()
            .fg(theme.faint)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        bar,
        Style::default().fg(theme.accent),
    )));
    lines.push(Line::from(Span::styled(
        format!("{} / {} · {:.0}%", tokens(used), tokens(limit), ratio * 100.0),
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));

    let cost = session.cost;
    lines.push(Line::from(Span::styled(
        "COST",
        Style::default()
            .fg(theme.faint)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("${cost:.4}"),
        Style::default().fg(theme.text),
    )));

    let footer_height = 3u16;
    let body_height = inner.height.saturating_sub(footer_height);
    let body = Rect::new(inner.x, inner.y, inner.width, body_height);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        body,
    );

    let footer_y = inner.y + inner.height.saturating_sub(footer_height);
    let mut footer: Vec<Line> = Vec::new();
    if session.running {
        footer.push(Line::from(vec![
            Span::styled(
                SPINNER[app.spinner % SPINNER.len()],
                Style::default().fg(theme.interactive),
            ),
            Span::styled(" working…", Style::default().fg(theme.muted)),
        ]));
    } else {
        footer.push(Line::from(Span::styled(
            "ready",
            Style::default().fg(theme.success),
        )));
    }
    footer.push(Line::from(Span::styled(
        "esc back · ctrl+p menu",
        Style::default().fg(theme.faint),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(footer)),
        Rect::new(inner.x, footer_y, inner.width, footer_height),
    );
}

fn transcript_lines(
    app: &App,
    session: &SessionView,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let last = session.messages.len().saturating_sub(1);
    for (index, message) in session.messages.iter().enumerate() {
        match message {
            Message::User(user) => user_lines(&mut out, user, width, theme),
            Message::Assistant(assistant) => {
                assistant_lines(
                    &mut out,
                    assistant,
                    width,
                    theme,
                    session,
                    app,
                    index == last && session.running,
                )
            }
            Message::Shell(shell) => shell_lines(&mut out, shell, width, theme, app),
            Message::System(system) => {
                out.push(Line::from(""));
                for line in crate::markdown::wrap_text(
                    &system.text,
                    width.saturating_sub(2),
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ) {
                    out.push(indent(line, 2));
                }
                out.push(Line::from(""));
            }
            Message::Skill(skill) => {
                out.push(Line::from(Span::styled(
                    format!("  skill · {}", skill.name),
                    Style::default().fg(theme.accent),
                )));
            }
            Message::Idle(idle) => {
                if idle.outcome == "failed" {
                    if let Some(error) = &idle.error {
                        out.push(Line::from(Span::styled(
                            format!("  ✕ {}", error.message),
                            Style::default().fg(theme.error),
                        )));
                    }
                }
            }
            Message::Compaction(_) => {
                out.push(Line::from(Span::styled(
                    "  ── context compacted ──",
                    Style::default().fg(theme.faint),
                )));
                out.push(Line::from(""));
            }
            Message::Synthetic(_)
            | Message::AgentSelected(_)
            | Message::ModelSelected(_)
            | Message::LocationSwitched(_) => {}
        }
    }
    out
}

fn user_lines(out: &mut Vec<Line<'static>>, user: &UserMessage, width: usize, theme: &Theme) {
    if !out.is_empty() {
        out.push(Line::from(""));
    }
    let text_width = width.saturating_sub(2);
    for line in crate::markdown::render(&user.text, text_width, theme) {
        out.push(prefixed(line, "┃ ", Style::default().fg(theme.accent)));
    }
    if !user.files.is_empty() {
        for file in &user.files {
            if let Some(name) = file
                .get("filename")
                .or_else(|| file.get("path"))
                .or_else(|| file.get("url"))
                .and_then(Value::as_str)
            {
                out.push(Line::from(vec![
                    Span::styled("  @ ", Style::default().fg(theme.accent)),
                    Span::styled(
                        truncate(name, width.saturating_sub(4)),
                        Style::default().fg(theme.muted),
                    ),
                ]));
            }
        }
    }
    out.push(Line::from(""));
}

fn assistant_lines(
    out: &mut Vec<Line<'static>>,
    assistant: &AssistantMessage,
    width: usize,
    theme: &Theme,
    session: &SessionView,
    app: &App,
    streaming: bool,
) {
    let text_width = width.saturating_sub(2);
    let mut wrote = false;
    for part in &assistant.content {
        match part {
            AssistantPart::Reasoning { text, .. } => {
                let text = text.replace("[REDACTED]", "");
                let text = text.trim();
                if text.is_empty() || !session.show_reasoning {
                    continue;
                }
                if wrote {
                    out.push(Line::from(""));
                }
                let done = assistant.time.completed.is_some();
                let mut header = vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        if done { "✓ " } else { "" },
                        Style::default().fg(theme.success),
                    ),
                ];
                if !done {
                    header.push(Span::styled(
                        SPINNER[app.spinner % SPINNER.len()],
                        Style::default().fg(theme.warning),
                    ));
                    header.push(Span::styled(" ", Style::default()));
                }
                header.push(Span::styled(
                    "Thought",
                    Style::default().fg(theme.warning),
                ));
                if done {
                    if let Some(completed) = assistant.time.completed {
                        let duration = (completed - assistant.time.created).max(0);
                        header.push(Span::styled(
                            format!(" · {}s", duration / 1000),
                            Style::default().fg(theme.faint),
                        ));
                    }
                }
                out.push(Line::from(header));
                for line in crate::markdown::wrap_text(
                    text,
                    width.saturating_sub(6),
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                ) {
                    out.push(prefixed(line, "  │ ", Style::default().fg(theme.border)));
                }
                wrote = true;
            }
            AssistantPart::Text { text, .. } => {
                if text.trim().is_empty() {
                    continue;
                }
                if wrote {
                    out.push(Line::from(""));
                }
                let rendered = if streaming {
                    crate::markdown::plain(text, text_width, theme)
                } else {
                    crate::markdown::render(text, text_width, theme)
                };
                for line in rendered {
                    out.push(indent(line, 2));
                }
                wrote = true;
            }
            AssistantPart::Tool(tool) => {
                if wrote {
                    out.push(Line::from(""));
                }
                tool_lines(out, tool, width, theme, app);
                wrote = true;
            }
        }
    }
    if let Some(error) = &assistant.error {
        out.push(Line::from(vec![
            Span::styled("  ✕ ", Style::default().fg(theme.error)),
            Span::styled(
                truncate(&error.message, width.saturating_sub(4)),
                Style::default().fg(theme.error),
            ),
        ]));
    }
    if wrote {
        out.push(Line::from(""));
    }
}

fn tool_lines(
    out: &mut Vec<Line<'static>>,
    tool: &ToolPart,
    width: usize,
    theme: &Theme,
    app: &App,
) {
    let (icon, color, input) = match &tool.state {
        ToolState::Streaming { input } => (
            SPINNER[app.spinner % SPINNER.len()].to_string(),
            theme.interactive,
            serde_json::from_str::<Value>(input).unwrap_or(Value::Null),
        ),
        ToolState::Running { input, .. } => (
            SPINNER[app.spinner % SPINNER.len()].to_string(),
            theme.interactive,
            input.clone(),
        ),
        ToolState::Completed { input, .. } => ("✓".to_string(), theme.success, input.clone()),
        ToolState::Error { input, .. } => ("✕".to_string(), theme.error, input.clone()),
    };

    let name = display_tool_name(&tool.name);
    let summary = tool_summary(&tool.name, &input);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            name,
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(summary) = summary {
        let remaining = width.saturating_sub(6 + width_of(&summary));
        let _ = remaining;
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            truncate(&summary, width.saturating_sub(6)),
            Style::default().fg(theme.muted),
        ));
    }
    out.push(Line::from(spans));

    match &tool.state {
        ToolState::Completed {
            content, metadata, ..
        } => {
            if matches!(tool.name.as_str(), "edit" | "write" | "patch") {
                let added = metadata
                    .get("additions")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let removed = metadata
                    .get("deletions")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if added > 0 || removed > 0 {
                    out.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(format!("+{added}"), Style::default().fg(theme.diff_added)),
                        Span::raw(" "),
                        Span::styled(format!("-{removed}"), Style::default().fg(theme.diff_removed)),
                    ]));
                }
                if let Some(patch) = metadata.get("patch").and_then(Value::as_str) {
                    for raw in patch.lines().take(8) {
                        let style = if raw.starts_with("+++") || raw.starts_with("---") {
                            Style::default().fg(theme.faint)
                        } else if raw.starts_with('+') {
                            Style::default().fg(theme.diff_added)
                        } else if raw.starts_with('-') {
                            Style::default().fg(theme.diff_removed)
                        } else if raw.starts_with("@@") {
                            Style::default().fg(theme.muted)
                        } else {
                            Style::default().fg(theme.diff_context)
                        };
                        out.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(truncate(raw, width.saturating_sub(4)), style),
                        ]));
                    }
                }
            } else if let Some(output) = content_text(content) {
                let lines: Vec<&str> = output.lines().take(4).collect();
                for raw in lines {
                    out.push(Line::from(vec![
                        Span::styled("    │ ", Style::default().fg(theme.border)),
                        Span::styled(
                            truncate(raw, width.saturating_sub(6)),
                            Style::default().fg(theme.muted),
                        ),
                    ]));
                }
                if output.lines().count() > 4 {
                    out.push(Line::from(Span::styled(
                        format!("    │ … {} more lines", output.lines().count() - 4),
                        Style::default().fg(theme.faint),
                    )));
                }
            }
        }
        ToolState::Error { error, .. } => {
            for line in crate::markdown::wrap_text(
                &error.message,
                width.saturating_sub(6),
                Style::default().fg(theme.error),
            ) {
                out.push(prefixed(line, "    │ ", Style::default().fg(theme.error)));
            }
        }
        _ => {}
    }
}

fn shell_lines(
    out: &mut Vec<Line<'static>>,
    shell: &ShellMessage,
    width: usize,
    theme: &Theme,
    app: &App,
) {
    out.push(Line::from(""));
    let running = shell.status == "running";
    out.push(Line::from(vec![
        Span::raw("  "),
        if running {
            Span::styled(
                format!("{} ", SPINNER[app.spinner % SPINNER.len()]),
                Style::default().fg(theme.interactive),
            )
        } else {
            Span::styled("$ ", Style::default().fg(theme.interactive))
        },
        Span::styled(
            truncate(&shell.command, width.saturating_sub(4)),
            Style::default().fg(theme.text),
        ),
    ]));
    if let Some(output) = &shell.output {
        let text = output
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| output.as_str().map(str::to_string))
            .unwrap_or_default();
        for raw in text.lines().take(5) {
            out.push(Line::from(vec![
                Span::styled("    │ ", Style::default().fg(theme.border)),
                Span::styled(
                    truncate(raw, width.saturating_sub(6)),
                    Style::default().fg(theme.muted),
                ),
            ]));
        }
    }
    out.push(Line::from(""));
}

fn draw_permission(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(session) = app.session.as_ref() else {
        return;
    };
    let Some(permission) = session.permission.as_ref() else {
        return;
    };

    let width = area.width.saturating_sub(8).min(76).max(28);
    let mut body: Vec<Line> = Vec::new();
    let action = permission
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    body.push(Line::from(vec![
        Span::styled(
            action.to_string(),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" wants permission", Style::default().fg(theme.muted)),
    ]));
    body.push(Line::from(""));
    if let Some(resources) = permission.get("resources").and_then(Value::as_array) {
        let joined = resources
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if !joined.is_empty() {
            for line in crate::markdown::wrap_text(
                &joined,
                width.saturating_sub(6) as usize,
                Style::default().fg(theme.interactive),
            ) {
                body.push(prefixed(line, "  ", Style::default()));
            }
        }
    }
    if let Some(description) = permission
        .get("metadata")
        .and_then(|metadata| metadata.get("description"))
        .and_then(Value::as_str)
    {
        body.push(Line::from(""));
        for line in crate::markdown::wrap_text(
            description,
            width.saturating_sub(6) as usize,
            Style::default().fg(theme.muted),
        ) {
            body.push(prefixed(line, "  ", Style::default()));
        }
    }
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::styled(" o ", Style::default().fg(theme.success).add_modifier(Modifier::BOLD)),
        Span::styled("allow once", Style::default().fg(theme.muted)),
        Span::styled("   a ", Style::default().fg(theme.interactive).add_modifier(Modifier::BOLD)),
        Span::styled("always", Style::default().fg(theme.muted)),
        Span::styled("   r ", Style::default().fg(theme.error).add_modifier(Modifier::BOLD)),
        Span::styled("reject", Style::default().fg(theme.muted)),
    ]));

    let height = (body.len() as u16 + 2).min(area.height.saturating_sub(4)).max(4);
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning))
        .style(Style::default().bg(theme.background_raised))
        .title(Span::styled(
            " Permission required ",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Text::from(body))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text)),
        inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
    );
}

fn draw_dialog(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(dialog) = app.dialog.as_ref() else {
        return;
    };
    match dialog {
        Dialog::List(list) => draw_list_dialog(frame, list, area, theme),
        Dialog::Help => draw_help_dialog(frame, area, theme),
        Dialog::Status => draw_status_dialog(frame, app, area, theme),
    }
}

fn dialog_frame(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    title: &str,
    width: u16,
    height: u16,
) -> Rect {
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background_raised))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

fn draw_list_dialog(frame: &mut Frame, list: &ListDialog, area: Rect, theme: &Theme) {
    let filtered: Vec<usize> = filtered_indices(list);
    let width = area.width.saturating_sub(8).min(88).max(30);
    let visible = filtered.len().clamp(1, 12) as u16;
    let height = (visible + 3).min(area.height.saturating_sub(4)).max(5);
    let inner = dialog_frame(frame, area, theme, &list.title, width, height);
    if inner.height < 3 || inner.width < 8 {
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(inner);
    let query = if list.query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.interactive)),
            Span::styled("type to search", Style::default().fg(theme.faint)),
        ])
    } else {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.interactive)),
            Span::styled(list.query.clone(), Style::default().fg(theme.text)),
        ])
    };
    frame.render_widget(Paragraph::new(query), rows[0]);
    let rule = "─".repeat(inner.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            rule,
            Style::default().fg(theme.border_subtle),
        ))),
        rows[1],
    );

    let list_area = rows[2];
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                list.empty.clone(),
                Style::default().fg(theme.muted),
            ))),
            list_area,
        );
        return;
    }

    let selected_pos = filtered
        .iter()
        .position(|index| *index == list.selected)
        .unwrap_or(0);
    let viewport = list_area.height as usize;
    let start = selected_pos
        .saturating_sub(viewport.saturating_sub(1))
        .min(filtered.len().saturating_sub(viewport));
    for (row, index) in filtered.iter().skip(start).take(viewport).enumerate() {
        let Some(item) = list.items.get(*index) else {
            continue;
        };
        let selected = *index == list.selected;
        let line = dialog_row(item, list_area.width as usize, selected, theme);
        let rect = Rect::new(list_area.x, list_area.y + row as u16, list_area.width, 1);
        let style = if selected {
            Style::default().bg(theme.background_element)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(line).style(style), rect);
    }
}

fn dialog_row(
    item: &DialogItem,
    width: usize,
    selected: bool,
    theme: &Theme,
) -> Line<'static> {
    let label_style = if item.disabled {
        Style::default().fg(theme.faint)
    } else if selected {
        Style::default()
            .fg(theme.interactive)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let label = truncate(&item.label, width.saturating_sub(6));
    let label_width = width_of(&label);
    let detail = truncate(&item.detail, width.saturating_sub(label_width + 6));
    let detail_width = width_of(&detail);
    let padding = width
        .saturating_sub(2 + label_width + 1 + detail_width)
        .max(1);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(label, label_style),
        Span::raw(" ".repeat(padding)),
    ];
    if !detail.is_empty() {
        spans.push(Span::styled(
            detail,
            Style::default().fg(if selected { theme.text } else { theme.faint }),
        ));
    }
    Line::from(spans)
}

fn draw_help_dialog(frame: &mut Frame, area: Rect, theme: &Theme) {
    let keys = [
        ("enter", "send prompt"),
        ("shift+enter", "new line"),
        ("ctrl+j", "new line"),
        ("tab", "switch agent"),
        ("shift+tab", "previous agent"),
        ("esc", "interrupt · clear · back"),
        ("ctrl+p", "command palette"),
        ("ctrl+o", "sessions"),
        ("ctrl+x", "leader (n, l, m, a, b, q)"),
        ("F1", "keyboard shortcuts"),
        ("F2", "model picker"),
        ("pgup/pgdn", "scroll transcript"),
        ("ctrl+c", "interrupt · quit twice"),
    ];
    let width = area.width.saturating_sub(8).min(68).max(30);
    let height = (keys.len() as u16 + 4).min(area.height.saturating_sub(4));
    let inner = dialog_frame(frame, area, theme, "Keyboard shortcuts", width, height);
    if inner.height < 2 {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for (key, description) in keys {
        let key = format!("{key:<14}");
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(key, Style::default().fg(theme.interactive)),
            Span::styled(description, Style::default().fg(theme.muted)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_status_dialog(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let model = app
        .status_model()
        .map(|model| model.display())
        .unwrap_or_else(|| "no model".to_string());
    let mut entries: Vec<(String, String)> = vec![
        ("server".into(), app.client.base.clone()),
        ("directory".into(), app.directory.display().to_string()),
        ("model".into(), model),
        ("agent".into(), app.status_agent()),
        ("events".into(), if app.connected { "connected".into() } else { "disconnected".into() }),
        ("version".into(), format!("oxidecode {VERSION}")),
    ];
    if let Some(session) = app.session.as_ref() {
        entries.push(("session".into(), session.id.clone()));
        entries.push(("messages".into(), session.messages.len().to_string()));
        entries.push((
            "context".into(),
            format!(
                "{} / {}",
                tokens(app.context_tokens()),
                tokens(session.context_limit)
            ),
        ));
    }
    let width = area.width.saturating_sub(8).min(76).max(30);
    let height = (entries.len() as u16 * 2 + 2).min(area.height.saturating_sub(4));
    let inner = dialog_frame(frame, area, theme, "Status", width, height);
    if inner.height < 2 {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    for (label, value) in entries {
        lines.push(Line::from(Span::styled(
            label.to_uppercase(),
            Style::default()
                .fg(theme.faint)
                .add_modifier(Modifier::BOLD),
        )));
        lines.extend(crate::markdown::wrap_text(
            &value,
            inner.width as usize,
            Style::default().fg(theme.text),
        ));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_toast(frame: &mut Frame, area: Rect, theme: &Theme, message: &str, error: bool) {
    if area.width < 16 || area.height < 4 {
        return;
    }
    let color = if error { theme.error } else { theme.success };
    let width = (width_of(message) as u16 + 4)
        .min(area.width.saturating_sub(4))
        .max(16);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width + 2),
        area.y + 1,
        width,
        3,
    );
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(theme.background_raised));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate(message, width.saturating_sub(4) as usize),
            Style::default().fg(color),
        ))),
        inner,
    );
}

fn filtered_indices(list: &ListDialog) -> Vec<usize> {
    let query = list.query.trim().to_lowercase();
    if query.is_empty() {
        return (0..list.items.len()).collect();
    }
    list.items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.label.to_lowercase().contains(&query) || item.detail.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width).max(1);
    let height = height.min(area.height).max(1);
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn indent(line: Line<'static>, columns: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(columns))];
    spans.extend(line.spans);
    Line::from(spans)
}

fn prefixed(line: Line<'static>, prefix: &'static str, style: Style) -> Line<'static> {
    let mut spans = vec![Span::styled(prefix, style)];
    spans.extend(line.spans);
    Line::from(spans)
}

fn display_tool_name(name: &str) -> String {
    match name {
        "bash" => "shell".to_string(),
        "read" => "read".to_string(),
        "write" => "write".to_string(),
        "edit" => "edit".to_string(),
        "grep" => "grep".to_string(),
        "glob" => "glob".to_string(),
        "list" => "list".to_string(),
        "webfetch" => "webfetch".to_string(),
        "todowrite" | "todoread" => "todos".to_string(),
        other => other.to_string(),
    }
}

fn tool_summary(name: &str, input: &Value) -> Option<String> {
    let field = |keys: &[&str]| -> Option<String> {
        for key in keys {
            if let Some(value) = input.get(*key).and_then(Value::as_str) {
                if !value.is_empty() {
                    return Some(value.lines().next().unwrap_or(value).to_string());
                }
            }
        }
        None
    };
    match name {
        "bash" => field(&["description", "command"]),
        "read" | "write" | "edit" | "patch" => field(&["filePath", "path"]).map(|path| shorten_path(&path)),
        "grep" => field(&["pattern"]),
        "glob" => field(&["pattern"]),
        "list" => field(&["path"]),
        "webfetch" => field(&["url"]),
        "todowrite" => Some("updating the todo list".to_string()),
        _ => field(&["description", "path", "pattern", "command", "url"]),
    }
}

fn content_text(content: &[Value]) -> Option<String> {
    let mut out = String::new();
    for item in content {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            out.push_str(text);
        } else if let Some(text) = item.as_str() {
            out.push_str(text);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn shorten_path(path: &str) -> String {
    let trimmed = path.trim_start_matches("./");
    if trimmed.len() <= 60 {
        trimmed.to_string()
    } else {
        let name = Path::new(trimmed)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| trimmed.to_string());
        format!("…/{name}")
    }
}

fn shorten_dir(directory: &Path) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return directory.display().to_string();
    };
    let home = Path::new(&home);
    if let Ok(rest) = directory.strip_prefix(home) {
        if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        }
    } else {
        directory.display().to_string()
    }
}

fn tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// Human-friendly relative timestamp for the session list.
pub fn relative_time(timestamp: i64) -> String {
    let now = oxidecode_core::ids::now_ms();
    let delta = (now - timestamp).max(0);
    if delta < 60_000 {
        "now".to_string()
    } else if delta < 3_600_000 {
        format!("{}m ago", delta / 60_000)
    } else if delta < 86_400_000 {
        format!("{}h ago", delta / 3_600_000)
    } else {
        format!("{}d ago", delta / 86_400_000)
    }
}

/// Renders the session list entry used by the picker (kept here so the app and
/// UI agree on formatting).
pub fn session_label(info: &SessionInfo) -> String {
    if info.title.is_empty() {
        info.id.clone()
    } else {
        info.title.clone()
    }
}

/// Small helper used by the status dialog for alignment.
pub fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    let current = width_of(text);
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
}

#[allow(dead_code)]
fn align_right(text: &str, width: usize) -> Line<'static> {
    Line::from(Span::styled(
        pad(text, width),
        Style::default(),
    ))
    .alignment(Alignment::Right)
}
