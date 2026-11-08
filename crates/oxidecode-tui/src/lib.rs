//! OxideCode terminal interface.
//!
//! `run` owns the terminal for the lifetime of the UI: it switches to the
//! alternate screen, enables raw mode and bracketed paste, drives the Ratatui
//! event loop and restores the terminal on exit or panic.

pub mod api;
pub mod app;
pub mod input;
pub mod logo;
pub mod markdown;
pub mod theme;
pub mod ui;

use api::Client;
use app::App;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::mpsc;
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Options {
    /// Base URL of the local OxideCode server, e.g. `http://127.0.0.1:4096`.
    pub base: String,
    /// Basic-auth password issued by `service get password`.
    pub password: String,
    /// Project directory the UI operates on.
    pub directory: String,
    /// Optional prompt submitted immediately on startup.
    pub prompt: Option<String>,
    /// Optional model (`provider/model` or plain model id).
    pub model: Option<String>,
    /// Optional agent id.
    pub agent: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            base: "http://127.0.0.1:4096".to_string(),
            password: String::new(),
            directory: std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            prompt: None,
            model: None,
            agent: None,
        }
    }
}

struct TerminalGuard {
    stdout: Stdout,
}

impl TerminalGuard {
    fn new(stdout: Stdout) -> Self {
        Self { stdout }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.stdout,
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

/// Runs the TUI until the user quits. Returns once the terminal is restored.
pub fn run(options: Options) -> anyhow::Result<()> {
    let client = Client::new(
        options.base.clone(),
        options.password.clone(),
        options.directory.clone(),
    );
    let (sender, receiver) = mpsc::channel();
    api::spawn_events(client.clone(), sender);

    let mut app = App::new(
        client,
        receiver,
        theme::Theme::default(),
        options.prompt.clone(),
        options.model.clone(),
        options.agent.clone(),
    );

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        crossterm::cursor::Hide,
        SetTitle("OxideCode")
    )?;
    let guard = TerminalGuard::new(io::stdout());
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, &mut app);
    drop(terminal);
    drop(guard);
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> anyhow::Result<()> {
    loop {
        app.poll_events();
        app.tick();

        if app.dirty {
            let mut cursor: Option<(u16, u16)> = None;
            terminal.draw(|frame| {
                cursor = ui::draw(frame, app);
                if let Some((x, y)) = cursor {
                    frame.set_cursor_position((x, y));
                }
            })?;
            app.dirty = false;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        let width = terminal.size().map(|size| size.width).unwrap_or(80);
                        app.on_key(key, width);
                    }
                }
                Event::Paste(text) => app.on_paste(&text),
                Event::Resize(_, _) => app.dirty = true,
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
