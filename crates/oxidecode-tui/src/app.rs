//! TUI application state and key/event handling.

use crate::api::{AgentInfo, Client, ModelChoice, ModelInfo, SessionSummary, UiEvent};
use crate::input::Input;
use crate::theme::Theme;
use oxidecode_core::session::{
    AssistantMessage, AssistantPart, Message, ModelRef, SessionInfo, ToolPart, ToolState,
    TokenUsage,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Session,
}

pub struct HomeState {
    pub input: Input,
    pub model: Option<ModelChoice>,
    pub agent: String,
    pub submitting: bool,
    pub placeholder: usize,
}

pub struct SessionView {
    pub id: String,
    pub title: String,
    pub model: Option<ModelChoice>,
    pub agent: String,
    pub messages: Vec<Message>,
    pub input: Input,
    pub running: bool,
    pub scroll: usize,
    pub permission: Option<Value>,
    pub error: Option<String>,
    pub started: i64,
    pub tokens: TokenUsage,
    pub cost: f64,
    pub context_limit: u64,
    pub show_reasoning: bool,
    pub show_sidebar: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum DialogValue {
    Model(ModelChoice),
    Agent(String),
    Session(String),
    Command(CommandId),
}

#[derive(Debug, Clone)]
pub struct DialogItem {
    pub label: String,
    pub detail: String,
    pub value: DialogValue,
    pub disabled: bool,
}

#[derive(Debug, Clone)]
pub struct ListDialog {
    pub title: String,
    pub query: String,
    pub selected: usize,
    pub items: Vec<DialogItem>,
    pub empty: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    NewSession,
    Sessions,
    Models,
    Agents,
    Compact,
    Interrupt,
    Home,
    Help,
    Status,
    ToggleSidebar,
    ToggleReasoning,
    Quit,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    List(ListDialog),
    Help,
    Status,
}

pub struct Toast {
    pub message: String,
    pub error: bool,
    pub created: Instant,
}

pub struct App {
    pub client: Client,
    pub theme: Theme,
    pub directory: PathBuf,
    pub screen: Screen,
    pub home: HomeState,
    pub session: Option<SessionView>,
    pub models: Vec<ModelInfo>,
    pub agents: Vec<AgentInfo>,
    pub sessions: Vec<SessionSummary>,
    pub dialog: Option<Dialog>,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    pub connected: bool,
    pub leader: Option<Instant>,
    pub ctrl_c: Option<Instant>,
    pub events: Receiver<UiEvent>,
    pub dirty: bool,
    pub spinner: usize,
    pub last_tick: Instant,
}

impl App {
    pub fn new(
        client: Client,
        events: Receiver<UiEvent>,
        theme: Theme,
        prompt: Option<String>,
        model: Option<String>,
        agent: Option<String>,
    ) -> Self {
        let directory = PathBuf::from(&client.directory);
        let models = client.models().unwrap_or_default();
        let agents = client.agents().unwrap_or_default();
        let sessions = client.sessions(50).unwrap_or_default();
        let selected_model = resolve_model(&client, &models, model.as_deref());
        let selected_agent = agent
            .filter(|id| agents.iter().any(|agent| &agent.id == id))
            .or_else(|| {
                if agents.iter().any(|agent| agent.id == "build") {
                    Some("build".to_string())
                } else {
                    agents
                        .iter()
                        .find(|agent| agent.mode != "subagent" && !agent.hidden)
                        .map(|agent| agent.id.clone())
                }
            })
            .unwrap_or_else(|| "build".to_string());
        let mut app = Self {
            client,
            theme,
            directory,
            screen: Screen::Home,
            home: HomeState {
                input: Input::new(),
                model: selected_model,
                agent: selected_agent,
                submitting: false,
                placeholder: 0,
            },
            session: None,
            models,
            agents,
            sessions,
            dialog: None,
            toast: None,
            should_quit: false,
            connected: true,
            leader: None,
            ctrl_c: None,
            events,
            dirty: true,
            spinner: 0,
            last_tick: Instant::now(),
        };
        if let Some(prompt) = prompt {
            if !prompt.trim().is_empty() {
                app.home.input.set_text(&prompt);
                app.submit_home();
            }
        }
        app
    }

    pub fn models_enabled(&self) -> Vec<&ModelInfo> {
        let enabled: Vec<&ModelInfo> = self.models.iter().filter(|model| model.enabled).collect();
        if enabled.is_empty() {
            self.models.iter().collect()
        } else {
            enabled
        }
    }

    pub fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            message: message.into(),
            error,
            created: Instant::now(),
        });
        self.dirty = true;
    }

    pub fn open_models(&mut self) {
        let mut items: Vec<DialogItem> = self
            .models_enabled()
            .into_iter()
            .map(|model| DialogItem {
                label: model.display(),
                detail: format!(
                    "{} · {}K context{}",
                    model.name,
                    model.limit.context / 1000,
                    if model.enabled { "" } else { " · unavailable" }
                ),
                value: DialogValue::Model(ModelChoice::from_model(model)),
                disabled: false,
            })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        self.dialog = Some(Dialog::List(ListDialog {
            title: "Select model".to_string(),
            query: String::new(),
            selected: 0,
            items,
            empty: "No models available. Configure a provider or set OPENCODE_API_KEY.".to_string(),
        }));
        self.dirty = true;
    }

    pub fn open_agents(&mut self) {
        let mut items: Vec<DialogItem> = self
            .agents
            .iter()
            .filter(|agent| !agent.hidden)
            .filter(|agent| agent.mode != "subagent")
            .map(|agent| DialogItem {
                label: agent.id.clone(),
                detail: if agent.description.is_empty() {
                    agent.name.clone()
                } else {
                    agent.description.clone()
                },
                value: DialogValue::Agent(agent.id.clone()),
                disabled: false,
            })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        self.dialog = Some(Dialog::List(ListDialog {
            title: "Select agent".to_string(),
            query: String::new(),
            selected: 0,
            items,
            empty: "No agents available.".to_string(),
        }));
        self.dirty = true;
    }

    pub fn open_sessions(&mut self) {
        if let Ok(sessions) = self.client.sessions(100) {
            self.sessions = sessions;
        }
        let items: Vec<DialogItem> = self
            .sessions
            .iter()
            .map(|session| DialogItem {
                label: if session.title.is_empty() {
                    "Untitled session".to_string()
                } else {
                    session.title.clone()
                },
                detail: format!(
                    "{} · {}",
                    crate::ui::relative_time(session.time.updated),
                    session.id
                ),
                value: DialogValue::Session(session.id.clone()),
                disabled: false,
            })
            .collect();
        self.dialog = Some(Dialog::List(ListDialog {
            title: "Sessions".to_string(),
            query: String::new(),
            selected: 0,
            items,
            empty: "No sessions yet.".to_string(),
        }));
        self.dirty = true;
    }

    pub fn open_commands(&mut self) {
        let commands = [
            (CommandId::NewSession, "New session", "Start a fresh session"),
            (CommandId::Sessions, "Sessions", "Switch to another session"),
            (CommandId::Models, "Switch model", "Change the active model"),
            (CommandId::Agents, "Switch agent", "Change the active agent"),
            (CommandId::Compact, "Compact session", "Summarise the conversation"),
            (CommandId::Interrupt, "Interrupt", "Stop the running turn"),
            (CommandId::ToggleSidebar, "Toggle sidebar", "Show or hide the sidebar"),
            (CommandId::ToggleReasoning, "Toggle reasoning", "Show or hide model thinking"),
            (CommandId::Home, "Home", "Back to the new-session screen"),
            (CommandId::Status, "Status", "Server and session information"),
            (CommandId::Help, "Help", "Keyboard shortcuts"),
            (CommandId::Quit, "Quit", "Exit OxideCode"),
        ];
        let items = commands
            .into_iter()
            .map(|(id, label, detail)| DialogItem {
                label: label.to_string(),
                detail: detail.to_string(),
                value: DialogValue::Command(id),
                disabled: false,
            })
            .collect();
        self.dialog = Some(Dialog::List(ListDialog {
            title: "Commands".to_string(),
            query: String::new(),
            selected: 0,
            items,
            empty: "No commands".to_string(),
        }));
        self.dirty = true;
    }

    pub fn open_help(&mut self) {
        self.dialog = Some(Dialog::Help);
        self.dirty = true;
    }

    pub fn open_status(&mut self) {
        self.dialog = Some(Dialog::Status);
        self.dirty = true;
    }

    pub fn close_dialog(&mut self) {
        self.dialog = None;
        self.dirty = true;
    }

    fn cycle_agent(&mut self, backwards: bool) {
        let ids: Vec<String> = self
            .agents
            .iter()
            .filter(|agent| !agent.hidden && agent.mode != "subagent")
            .map(|agent| agent.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = self
            .session
            .as_ref()
            .map(|session| session.agent.clone())
            .unwrap_or_else(|| self.home.agent.clone());
        let index = ids.iter().position(|id| id == &current).unwrap_or(0);
        let next = if backwards {
            (index + ids.len() - 1) % ids.len()
        } else {
            (index + 1) % ids.len()
        };
        self.set_agent(ids[next].clone());
    }

    fn set_agent(&mut self, agent: String) {
        if let Some(session) = self.session.as_mut() {
            session.agent = agent.clone();
            let _ = self.client.switch_agent(&session.id, &agent);
        }
        self.home.agent = agent.clone();
        self.toast(format!("agent · {agent}"), false);
        self.dirty = true;
    }

    fn set_model(&mut self, model: ModelChoice) {
        if let Some(session) = self.session.as_mut() {
            let _ = self.client.switch_model(&session.id, &model);
            session.model = Some(model.clone());
        }
        self.home.model = Some(model.clone());
        self.toast(format!("model · {}", model.display()), false);
        self.dirty = true;
    }

    pub fn visible_sidebar(&self, width: u16) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        match session.show_sidebar {
            Some(value) => value,
            None => width > 120,
        }
    }

    pub fn toggle_sidebar(&mut self) {
        if let Some(session) = self.session.as_mut() {
            let current = session.show_sidebar.unwrap_or(false);
            session.show_sidebar = Some(!current);
            self.toast(
                if current {
                    "sidebar hidden"
                } else {
                    "sidebar shown"
                },
                false,
            );
            self.dirty = true;
        }
    }

    pub fn submit_home(&mut self) {
        let text = self.home.input.text();
        let text = text.trim_end().to_string();
        if text.trim().is_empty() || self.home.submitting {
            return;
        }
        if text.starts_with('/') && !text.contains('\n') {
            self.home.input.clear();
            self.handle_command(&text);
            return;
        }
        self.home.submitting = true;
        let created = self.client.create_session(
            None,
            self.home.model.as_ref(),
            Some(&self.home.agent),
        );
        self.home.submitting = false;
        match created {
            Ok(session) => {
                self.home.input.clear();
                self.enter_session(session);
                if let Some(session) = self.session.as_ref() {
                    let id = session.id.clone();
                    let _ = self.client.prompt(&id, &text);
                    self.set_running(true);
                    self.refresh_messages();
                    self.refresh_meta();
                }
            }
            Err(error) => self.toast(format!("session: {error}"), true),
        }
        self.dirty = true;
    }

    pub fn submit_session(&mut self) {
        let Some(session) = self.session.as_mut() else {
            self.submit_home();
            return;
        };
        let text = session.input.text();
        let text = text.trim_end().to_string();
        if text.trim().is_empty() {
            return;
        }
        if text.starts_with('/') && !text.contains('\n') {
            session.input.clear();
            self.handle_command(&text);
            return;
        }
        let id = session.id.clone();
        let queued = session.running;
        session.input.clear();
        match self.client.prompt(&id, &text) {
            Ok(_) => {
                if queued {
                    self.toast("queued — the agent will pick it up next", false);
                }
                self.set_running(true);
                self.refresh_messages();
            }
            Err(error) => self.toast(format!("prompt: {error}"), true),
        }
        self.dirty = true;
    }

    fn handle_command(&mut self, text: &str) {
        let mut parts = text.trim().splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("").to_string();
        let args = parts.next().unwrap_or("").trim().to_string();
        match command.as_str() {
            "/new" => self.new_session(),
            "/sessions" | "/resume" | "/session" => self.open_sessions(),
            "/models" | "/model" => {
                if !args.is_empty() {
                    if let Some(current) = self.models_enabled().into_iter().find(|model| {
                        model.display() == args
                            || model.id == args
                            || model.name.eq_ignore_ascii_case(&args)
                    }) {
                        self.set_model(ModelChoice::from_model(current));
                    } else {
                        self.toast(format!("unknown model · {args}"), true);
                    }
                } else {
                    self.open_models();
                }
            }
            "/agents" | "/agent" => {
                if !args.is_empty() {
                    if self.agents.iter().any(|agent| agent.id == args) {
                        self.set_agent(args);
                    } else {
                        self.toast(format!("unknown agent · {args}"), true);
                    }
                } else {
                    self.open_agents();
                }
            }
            "/compact" => {
                if let Some(session) = &self.session {
                    let id = session.id.clone();
                    match self.client.compact(&id) {
                        Ok(_) => self.toast("compaction started", false),
                        Err(error) => self.toast(format!("compact: {error}"), true),
                    }
                } else {
                    self.toast("compact is only available inside a session", true);
                }
            }
            "/init" => {
                let prompt = "Analyse this project and create an AGENTS.md file describing how to build, test and work in this codebase. Keep it concise and useful for future sessions.";
                if let Some(session) = self.session.as_mut() {
                    let id = session.id.clone();
                    let _ = self.client.prompt(&id, prompt);
                    self.set_running(true);
                    self.refresh_messages();
                } else {
                    self.home.input.set_text(prompt);
                    self.submit_home();
                }
            }
            "/help" => self.open_help(),
            "/status" => self.open_status(),
            "/home" => self.go_home(),
            "/quit" | "/exit" => self.should_quit = true,
            "/undo" => self.go_home(),
            other => self.toast(format!("unknown command · {other}"), true),
        }
        self.dirty = true;
    }

    pub fn new_session(&mut self) {
        self.session = None;
        self.screen = Screen::Home;
        self.home.input.clear();
        self.dirty = true;
    }

    pub fn go_home(&mut self) {
        self.session = None;
        self.screen = Screen::Home;
        self.dirty = true;
    }

    fn enter_session(&mut self, info: SessionInfo) {
        let id = info.id.clone();
        let model = info.model.as_ref().map(|model| ModelChoice {
            provider_id: model.provider_id.clone(),
            id: model.id.clone(),
            variant: model.variant.clone(),
        });
        let model = model.or_else(|| self.home.model.clone());
        let agent = info.agent.clone().unwrap_or_else(|| self.home.agent.clone());
        let mut view = SessionView {
            id: id.clone(),
            title: info.title.clone(),
            model: model.clone(),
            agent: agent.clone(),
            messages: Vec::new(),
            input: Input::new(),
            running: false,
            scroll: 0,
            permission: None,
            error: None,
            started: info.time.created,
            tokens: info.tokens.clone(),
            cost: info.cost,
            context_limit: self.context_limit(model.as_ref()),
            show_reasoning: true,
            show_sidebar: None,
        };
        if let Ok(messages) = self.client.messages(&id) {
            view.messages = messages;
        }
        if let Ok(active) = self.client.get("/api/session/active") {
            if active
                .get("data")
                .and_then(|data| data.get(&id))
                .is_some()
            {
                view.running = true;
            }
        }
        if let Ok(permissions) = self.client.permissions(&id) {
            view.permission = permissions
                .into_iter()
                .filter(|permission| permission.get("decision").map(Value::is_null).unwrap_or(true))
                .next_back();
        }
        self.session = Some(view);
        self.screen = Screen::Session;
        self.home.model = model;
        self.home.agent = agent;
        self.dirty = true;
        let _ = self.client.post(&format!("/api/session/{id}/view"), json!({}));
    }

    fn context_limit(&self, model: Option<&ModelChoice>) -> u64 {
        context_limit_for(&self.models, model)
    }

    fn open_session(&mut self, id: &str) {
        match self.client.session(id) {
            Ok(info) => {
                self.enter_session(info);
                self.refresh_meta();
            }
            Err(error) => self.toast(format!("session: {error}"), true),
        }
    }

    pub fn refresh_messages(&mut self) {
        let Some(id) = self.session.as_ref().map(|session| session.id.clone()) else {
            return;
        };
        if let Ok(messages) = self.client.messages(&id) {
            if let Some(session) = self.session.as_mut() {
                session.messages = messages;
            }
            self.dirty = true;
        }
    }

    pub fn refresh_meta(&mut self) {
        let Some(id) = self.session.as_ref().map(|session| session.id.clone()) else {
            return;
        };
        if let Ok(info) = self.client.session(&id) {
            if let Some(session) = self.session.as_mut() {
                session.title = info.title;
                session.tokens = info.tokens;
                session.cost = info.cost;
                session.agent = info.agent.unwrap_or(session.agent.clone());
                if let Some(model) = info.model {
                    session.model = Some(ModelChoice {
                        provider_id: model.provider_id,
                        id: model.id,
                        variant: model.variant,
                    });
                }
                session.context_limit =
                    context_limit_for(&self.models, session.model.as_ref());
            }
            self.dirty = true;
        }
    }

    fn set_running(&mut self, running: bool) {
        if let Some(session) = self.session.as_mut() {
            session.running = running;
            if running {
                session.scroll = 0;
            }
        }
        self.dirty = true;
    }

    pub fn interrupt(&mut self) {
        if let Some(session) = &self.session {
            if session.running {
                let id = session.id.clone();
                let _ = self.client.interrupt(&id);
                self.toast("interrupting…", false);
            }
        }
    }

    fn respond_permission(&mut self, decision: &str) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(permission) = session.permission.clone() else {
            return;
        };
        let Some(id) = permission.get("id").and_then(Value::as_str) else {
            return;
        };
        let session_id = session.id.clone();
        let id = id.to_string();
        match self.client.reply_permission(&session_id, &id, decision) {
            Ok(_) => {
                session.permission = None;
                self.toast(
                    match decision {
                        "always" => "allowed (always)",
                        "once" => "allowed",
                        _ => "rejected",
                    },
                    false,
                );
            }
            Err(error) => self.toast(format!("permission: {error}"), true),
        }
        self.dirty = true;
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.dialog.is_some() {
            return;
        }
        if self.screen == Screen::Home {
            self.home.input.insert_str(text);
        } else if let Some(session) = self.session.as_mut() {
            session.input.insert_str(text);
        }
        self.dirty = true;
    }

    pub fn on_key(&mut self, key: KeyEvent, width: u16) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Leader key: ctrl+x followed by one of the OpenCode shortcuts.
        if ctrl && matches!(key.code, KeyCode::Char('x')) {
            self.leader = Some(Instant::now());
            self.toast("leader … (n new · l sessions · m model · a agent · b sidebar · q quit)", false);
            return;
        }
        if let Some(started) = self.leader.take() {
            if started.elapsed() < Duration::from_secs(3) {
                if self.handle_leader_key(key) {
                    return;
                }
            }
        }

        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            if self.dialog.is_some() {
                self.close_dialog();
                return;
            }
            if self.session.as_ref().map(|session| session.running).unwrap_or(false) {
                self.interrupt();
                self.ctrl_c = Some(Instant::now());
                return;
            }
            let input_empty = match self.screen {
                Screen::Home => self.home.input.is_empty(),
                Screen::Session => self
                    .session
                    .as_ref()
                    .map(|session| session.input.is_empty())
                    .unwrap_or(true),
            };
            if !input_empty {
                match self.screen {
                    Screen::Home => self.home.input.clear(),
                    Screen::Session => {
                        if let Some(session) = self.session.as_mut() {
                            session.input.clear();
                        }
                    }
                }
                self.ctrl_c = Some(Instant::now());
                return;
            }
            if self
                .ctrl_c
                .map(|started| started.elapsed() < Duration::from_secs(2))
                .unwrap_or(false)
            {
                self.should_quit = true;
                return;
            }
            self.ctrl_c = Some(Instant::now());
            self.toast("press ctrl+c again to exit", false);
            return;
        }

        if self.dialog.is_some() {
            self.handle_dialog_key(key);
            return;
        }

        // Permission prompts capture the keyboard.
        if self
            .session
            .as_ref()
            .map(|session| session.permission.is_some())
            .unwrap_or(false)
        {
            match key.code {
                KeyCode::Char('o') | KeyCode::Enter => self.respond_permission("once"),
                KeyCode::Char('a') => self.respond_permission("always"),
                KeyCode::Char('r') | KeyCode::Esc => self.respond_permission("reject"),
                _ => {}
            }
            return;
        }

        if ctrl && matches!(key.code, KeyCode::Char('p')) {
            self.open_commands();
            return;
        }
        if ctrl && matches!(key.code, KeyCode::Char('o')) {
            self.open_sessions();
            return;
        }
        if matches!(key.code, KeyCode::F(2)) {
            self.open_models();
            return;
        }
        if matches!(key.code, KeyCode::F(1)) {
            self.open_help();
            return;
        }

        if matches!(key.code, KeyCode::Tab) && !shift {
            self.cycle_agent(false);
            return;
        }
        if matches!(key.code, KeyCode::BackTab) {
            self.cycle_agent(true);
            return;
        }

        if key.code == KeyCode::Esc {
            if self.session.as_ref().map(|session| session.running).unwrap_or(false) {
                self.interrupt();
            } else {
                let empty = match self.screen {
                    Screen::Home => self.home.input.is_empty(),
                    Screen::Session => self
                        .session
                        .as_ref()
                        .map(|session| session.input.is_empty())
                        .unwrap_or(true),
                };
                if !empty {
                    match self.screen {
                        Screen::Home => self.home.input.clear(),
                        Screen::Session => {
                            if let Some(session) = self.session.as_mut() {
                                session.input.clear();
                            }
                        }
                    }
                } else if self.screen == Screen::Session {
                    self.go_home();
                }
            }
            self.dirty = true;
            return;
        }

        match self.screen {
            Screen::Home => self.home_key(key, width, ctrl, alt, shift),
            Screen::Session => self.session_key(key, width, ctrl, alt, shift),
        }
    }

    fn handle_leader_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('n') => self.new_session(),
            KeyCode::Char('l') => self.open_sessions(),
            KeyCode::Char('m') => self.open_models(),
            KeyCode::Char('a') => self.open_agents(),
            KeyCode::Char('b') => self.toggle_sidebar(),
            KeyCode::Char('p') => self.open_commands(),
            KeyCode::Char('s') => self.open_status(),
            KeyCode::Char('q') | KeyCode::Char('c') => self.should_quit = true,
            KeyCode::Char('h') => self.go_home(),
            KeyCode::Esc => {}
            _ => return false,
        }
        true
    }

    fn home_key(&mut self, key: KeyEvent, width: u16, ctrl: bool, alt: bool, shift: bool) {
        let input_width = width.saturating_sub(4) as usize;
        match key.code {
            KeyCode::Enter if !shift && !alt => self.submit_home(),
            KeyCode::Enter if ctrl => {
                self.home.input.insert_char('\n');
            }
            KeyCode::Enter => {
                self.home.input.insert_char('\n');
            }
            _ => {
                if edit_input(&mut self.home.input, &key, input_width, ctrl, alt, shift) {
                    self.dirty = true;
                }
            }
        }
    }

    fn session_key(&mut self, key: KeyEvent, width: u16, ctrl: bool, alt: bool, shift: bool) {
        let input_width = width.saturating_sub(4) as usize;
        if matches!(key.code, KeyCode::PageUp) {
            if let Some(session) = self.session.as_mut() {
                session.scroll = session.scroll.saturating_add(10);
            }
            self.dirty = true;
            return;
        }
        if matches!(key.code, KeyCode::PageDown) {
            if let Some(session) = self.session.as_mut() {
                session.scroll = session.scroll.saturating_sub(10);
            }
            self.dirty = true;
            return;
        }
        if ctrl && matches!(key.code, KeyCode::Up) {
            if let Some(session) = self.session.as_mut() {
                session.scroll = session.scroll.saturating_add(3);
            }
            self.dirty = true;
            return;
        }
        if ctrl && matches!(key.code, KeyCode::Down) {
            if let Some(session) = self.session.as_mut() {
                session.scroll = session.scroll.saturating_sub(3);
            }
            self.dirty = true;
            return;
        }
        match key.code {
            KeyCode::Enter if !shift && !alt => self.submit_session(),
            KeyCode::Enter if ctrl => {
                if let Some(session) = self.session.as_mut() {
                    session.input.insert_char('\n');
                    session.scroll = 0;
                }
            }
            KeyCode::Enter => {
                if let Some(session) = self.session.as_mut() {
                    session.input.insert_char('\n');
                    session.scroll = 0;
                }
            }
            _ => {
                if let Some(session) = self.session.as_mut() {
                    if edit_input(&mut session.input, &key, input_width, ctrl, alt, shift) {
                        self.dirty = true;
                    }
                }
            }
        }
    }

    fn handle_dialog_key(&mut self, key: KeyEvent) {
        if self.dialog.is_none() {
            return;
        }
        if !matches!(self.dialog, Some(Dialog::List(_))) {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')
            ) {
                self.close_dialog();
            }
            return;
        }

        enum Action {
            None,
            Close,
            Activate(DialogValue),
        }
        let mut action = Action::None;
        if let Some(Dialog::List(list)) = self.dialog.as_mut() {
            let filtered = filtered_indices(list);
            match key.code {
                KeyCode::Esc => action = Action::Close,
                KeyCode::Up => {
                    if !filtered.is_empty() {
                        let position = filtered
                            .iter()
                            .position(|index| *index == list.selected)
                            .unwrap_or(0);
                        let next = if position == 0 {
                            filtered.len() - 1
                        } else {
                            position - 1
                        };
                        list.selected = filtered[next];
                    }
                }
                KeyCode::Down => {
                    if !filtered.is_empty() {
                        let position = filtered
                            .iter()
                            .position(|index| *index == list.selected)
                            .unwrap_or(0);
                        let next = (position + 1) % filtered.len();
                        list.selected = filtered[next];
                    }
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !filtered.is_empty() {
                        let position = filtered
                            .iter()
                            .position(|index| *index == list.selected)
                            .unwrap_or(0);
                        let next = (position + 1) % filtered.len();
                        list.selected = filtered[next];
                    }
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !filtered.is_empty() {
                        let position = filtered
                            .iter()
                            .position(|index| *index == list.selected)
                            .unwrap_or(0);
                        let next = if position == 0 {
                            filtered.len() - 1
                        } else {
                            position - 1
                        };
                        list.selected = filtered[next];
                    }
                }
                KeyCode::Enter => {
                    if let Some(item) = list.items.get(list.selected).cloned() {
                        if !item.disabled {
                            action = Action::Activate(item.value);
                        }
                    }
                }
                KeyCode::Backspace => {
                    list.query.pop();
                    select_first(list);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    list.query.clear();
                    select_first(list);
                }
                KeyCode::Char(ch)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    list.query.push(ch);
                    select_first(list);
                }
                _ => {}
            }
        }
        self.dirty = true;
        match action {
            Action::None => {}
            Action::Close => self.close_dialog(),
            Action::Activate(value) => self.activate(value),
        }
    }

    fn activate(&mut self, value: DialogValue) {
        self.close_dialog();
        match value {
            DialogValue::Model(model) => self.set_model(model),
            DialogValue::Agent(agent) => self.set_agent(agent),
            DialogValue::Session(id) => self.open_session(&id),
            DialogValue::Command(command) => match command {
                CommandId::NewSession => self.new_session(),
                CommandId::Sessions => self.open_sessions(),
                CommandId::Models => self.open_models(),
                CommandId::Agents => self.open_agents(),
                CommandId::Compact => {
                    if let Some(session) = &self.session {
                        let id = session.id.clone();
                        match self.client.compact(&id) {
                            Ok(_) => self.toast("compaction started", false),
                            Err(error) => self.toast(format!("compact: {error}"), true),
                        }
                    } else {
                        self.toast("compact is only available inside a session", true);
                    }
                }
                CommandId::Interrupt => self.interrupt(),
                CommandId::Home => self.go_home(),
                CommandId::Help => self.open_help(),
                CommandId::Status => self.open_status(),
                CommandId::ToggleSidebar => self.toggle_sidebar(),
                CommandId::ToggleReasoning => {
                    if let Some(session) = self.session.as_mut() {
                        session.show_reasoning = !session.show_reasoning;
                    }
                    self.dirty = true;
                }
                CommandId::Quit => self.should_quit = true,
            },
        }
    }

    /// Drains events from the SSE thread.
    pub fn poll_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.handle_ui_event(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.connected = false;
                    break;
                }
            }
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Connected => {
                self.connected = true;
                self.dirty = true;
                if self.session.is_some() {
                    self.refresh_messages();
                    self.refresh_meta();
                }
            }
            UiEvent::Disconnected(message) => {
                self.connected = false;
                self.toast(format!("event stream: {message}"), true);
            }
            UiEvent::Event(value) => self.handle_event(value),
        }
    }

    fn handle_event(&mut self, event: Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        let session_id = data.get("sessionID").and_then(Value::as_str);
        let current = self.session.as_ref().map(|session| session.id.clone());

        if kind == "server.connected" {
            self.connected = true;
            self.dirty = true;
            return;
        }

        // Session-independent events still update the cached session list/title.
        match kind {
            "session.renamed" => {
                if let (Some(id), Some(title)) = (
                    session_id,
                    data.get("title").and_then(Value::as_str),
                ) {
                    if let Some(session) = self.session.as_mut() {
                        if session.id == id {
                            session.title = title.to_string();
                            self.dirty = true;
                        }
                    }
                    if let Some(entry) = self.sessions.iter_mut().find(|entry| entry.id == id) {
                        entry.title = title.to_string();
                    }
                }
                return;
            }
            "session.created" | "session.deleted" => {
                if let Ok(sessions) = self.client.sessions(50) {
                    self.sessions = sessions;
                }
                return;
            }
            _ => {}
        }

        let Some(current_id) = current else {
            return;
        };
        if session_id != Some(current_id.as_str()) {
            return;
        }

        match kind {
            "session.execution.started" | "session.step.started" => {
                self.set_running(true);
            }
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
            | "session.idle" => {
                self.set_running(false);
                if kind == "session.execution.failed" {
                    if let Some(message) = data
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                    {
                        self.toast(message.to_string(), true);
                    }
                }
                self.refresh_messages();
                self.refresh_meta();
            }
            "session.text.delta" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let delta = data.get("delta").and_then(Value::as_str).unwrap_or("");
                self.with_assistant(id, |assistant| assistant.push_text(delta));
                self.set_running(true);
            }
            "session.text.ended" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text = data.get("text").and_then(Value::as_str).unwrap_or("");
                let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.with_assistant(id, |assistant| {
                    set_text_part(assistant, ordinal, text);
                });
            }
            "session.reasoning.delta" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let delta = data.get("delta").and_then(Value::as_str).unwrap_or("");
                self.with_assistant(id, |assistant| assistant.push_reasoning(delta));
            }
            "session.reasoning.ended" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text = data.get("text").and_then(Value::as_str).unwrap_or("");
                let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.with_assistant(id, |assistant| {
                    set_reasoning_part(assistant, ordinal, text);
                });
            }
            "session.tool.input.started" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_id = data.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = data.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                self.with_assistant(id, |assistant| {
                    if !assistant.content.iter().any(|part| {
                        matches!(part, AssistantPart::Tool(tool) if tool.id == tool_id)
                    }) {
                        assistant.content.push(AssistantPart::Tool(ToolPart::new(
                            tool_id,
                            name,
                            String::new(),
                        )));
                    }
                });
            }
            "session.tool.input.delta" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
                let delta = data.get("delta").and_then(Value::as_str).unwrap_or("");
                self.with_assistant(id, |assistant| {
                    if let Some(tool) = tool_mut(assistant, tool_id) {
                        if let ToolState::Streaming { input } = &mut tool.state {
                            input.push_str(delta);
                        }
                    }
                });
            }
            "session.tool.input.ended" | "session.tool.called" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = data.get("name").and_then(Value::as_str).unwrap_or_default();
                let input = data
                    .get("input")
                    .cloned()
                    .or_else(|| {
                        data.get("text")
                            .and_then(Value::as_str)
                            .and_then(|text| serde_json::from_str(text).ok())
                    })
                    .unwrap_or_else(|| json!({}));
                self.with_assistant(id, |assistant| {
                    if let Some(tool) = tool_mut(assistant, tool_id) {
                        tool.name = name.to_string();
                        if matches!(tool.state, ToolState::Streaming { .. }) {
                            tool.state = ToolState::Running {
                                input: input.clone(),
                                metadata: json!({}),
                            };
                        }
                    }
                });
            }
            "session.tool.success" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
                let content = data
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let metadata = data.get("metadata").cloned().unwrap_or_else(|| json!({}));
                self.with_assistant(id, |assistant| {
                    if let Some(tool) = tool_mut(assistant, tool_id) {
                        tool.executed = true;
                        tool.time.ran = Some(oxidecode_core::ids::now_ms());
                        tool.time.completed = Some(oxidecode_core::ids::now_ms());
                        tool.state = ToolState::Completed {
                            input: tool_input(&tool.state),
                            content: content.clone(),
                            metadata: metadata.clone(),
                        };
                    }
                });
                self.dirty = true;
            }
            "session.tool.failed" => {
                let id = data
                    .get("assistantMessageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
                let content = data
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let error = data.get("error").cloned().unwrap_or_else(|| json!({
                    "type": "ToolExecutionError",
                    "message": "tool failed"
                }));
                self.with_assistant(id, |assistant| {
                    if let Some(tool) = tool_mut(assistant, tool_id) {
                        tool.executed = true;
                        tool.time.ran = Some(oxidecode_core::ids::now_ms());
                        tool.time.completed = Some(oxidecode_core::ids::now_ms());
                        let structured = serde_json::from_value(error.clone())
                            .unwrap_or_else(|_| oxidecode_core::session::StructuredError::new(
                                "ToolExecutionError",
                                error
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("tool failed"),
                            ));
                        tool.state = ToolState::Error {
                            input: tool_input(&tool.state),
                            error: structured,
                            content: content.clone(),
                            metadata: json!({}),
                        };
                    }
                });
                self.dirty = true;
            }
            "session.tool.progress" => {
                self.dirty = true;
            }
            "permission.asked" => {
                if let Some(session) = self.session.as_mut() {
                    session.permission = Some(data.clone());
                    session.running = true;
                }
                self.toast("permission required", false);
            }
            "permission.replied" => {
                if let Some(session) = self.session.as_mut() {
                    let replied = data.get("requestID").and_then(Value::as_str);
                    let current = session
                        .permission
                        .as_ref()
                        .and_then(|permission| permission.get("id"))
                        .and_then(Value::as_str);
                    if replied == current {
                        session.permission = None;
                    }
                }
            }
            "session.model.selected" => {
                if let Some(model) = data.get("model") {
                    if let (Some(provider), Some(id)) = (
                        model.get("providerID").and_then(Value::as_str),
                        model.get("id").and_then(Value::as_str),
                    ) {
                        if let Some(session) = self.session.as_mut() {
                            session.model = Some(ModelChoice {
                                provider_id: provider.to_string(),
                                id: id.to_string(),
                                variant: model
                                    .get("variant")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                            });
                        }
                    }
                }
            }
            "session.agent.selected" => {
                if let Some(agent) = data.get("agent").and_then(Value::as_str) {
                    if let Some(session) = self.session.as_mut() {
                        session.agent = agent.to_string();
                    }
                }
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn with_assistant(&mut self, message_id: &str, update: impl FnOnce(&mut AssistantMessage)) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if message_id.is_empty() {
            return;
        }
        let position = session.messages.iter().position(|message| {
            matches!(message, Message::Assistant(assistant) if assistant.id == message_id)
        });
        let index = match position {
            Some(index) => index,
            None => {
                let model = session
                    .model
                    .clone()
                    .map(|model| ModelRef {
                        provider_id: model.provider_id,
                        id: model.id,
                        variant: model.variant,
                    })
                    .unwrap_or_else(|| ModelRef::new("unknown", "unknown"));
                let assistant = AssistantMessage::new(session.agent.clone(), model);
                let mut assistant = assistant;
                assistant.id = message_id.to_string();
                session.messages.push(Message::Assistant(assistant));
                session.messages.len() - 1
            }
        };
        if let Message::Assistant(assistant) = &mut session.messages[index] {
            update(assistant);
        }
        self.dirty = true;
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(toast) = &self.toast {
            if now.duration_since(toast.created) > Duration::from_secs(5) {
                self.toast = None;
                self.dirty = true;
            }
        }
        if let Some(started) = self.leader {
            if now.duration_since(started) > Duration::from_secs(3) {
                self.leader = None;
            }
        }
        if self.session.as_ref().map(|session| session.running).unwrap_or(false) {
            if now.duration_since(self.last_tick) > Duration::from_millis(120) {
                self.spinner = self.spinner.wrapping_add(1);
                self.last_tick = now;
                self.dirty = true;
            }
        }
    }

    pub fn status_model(&self) -> Option<ModelChoice> {
        self.session
            .as_ref()
            .and_then(|session| session.model.clone())
            .or_else(|| self.home.model.clone())
    }

    pub fn status_agent(&self) -> String {
        self.session
            .as_ref()
            .map(|session| session.agent.clone())
            .unwrap_or_else(|| self.home.agent.clone())
    }

    pub fn context_tokens(&self) -> u64 {
        self.session
            .as_ref()
            .map(|session| {
                session
                    .tokens
                    .input
                    .saturating_add(session.tokens.output)
                    .saturating_add(session.tokens.reasoning)
                    .saturating_add(session.tokens.cache.read)
            })
            .unwrap_or(0)
    }
}

fn context_limit_for(models: &[ModelInfo], model: Option<&ModelChoice>) -> u64 {
    let Some(model) = model else {
        return 200_000;
    };
    models
        .iter()
        .find(|info| info.provider_id == model.provider_id && info.id == model.id)
        .map(|info| info.limit.context)
        .filter(|value| *value > 0)
        .unwrap_or(200_000)
}

fn resolve_model(client: &Client, models: &[ModelInfo], wanted: Option<&str>) -> Option<ModelChoice> {    if let Some(wanted) = wanted {
        let wanted = wanted.trim();
        if let Some(found) = models.iter().find(|model| {
            model.display() == wanted || model.id == wanted || model.name.eq_ignore_ascii_case(wanted)
        }) {
            return Some(ModelChoice::from_model(found));
        }
        if let Some((provider, rest)) = wanted.split_once('/') {
            let (id, variant) = match rest.split_once('#') {
                Some((id, variant)) if !variant.is_empty() => (id, Some(variant.to_string())),
                _ => (rest, None),
            };
            return Some(ModelChoice {
                provider_id: provider.to_string(),
                id: id.to_string(),
                variant,
            });
        }
    }
    if let Ok(value) = client.get("/api/model/default") {
        if let Some(data) = value.get("data").filter(|value| !value.is_null()) {
            if let Ok(model) = serde_json::from_value::<ModelInfo>(data.clone()) {
                return Some(ModelChoice::from_model(&model));
            }
        }
    }
    models
        .iter()
        .find(|model| model.enabled)
        .or_else(|| models.first())
        .map(ModelChoice::from_model)
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

fn select_first(list: &mut ListDialog) {
    let filtered = filtered_indices(list);
    list.selected = filtered.first().copied().unwrap_or(0);
}

fn edit_input(
    input: &mut Input,
    key: &KeyEvent,
    width: usize,
    ctrl: bool,
    alt: bool,
    _shift: bool,
) -> bool {
    match key.code {
        KeyCode::Char('j') if ctrl => input.insert_char('\n'),
        KeyCode::Char('a') if ctrl => input.home(),
        KeyCode::Char('e') if ctrl => input.end(),
        KeyCode::Char('w') if ctrl => input.delete_word_back(),
        KeyCode::Char('u') if ctrl => input.delete_line(),
        KeyCode::Char('k') if ctrl => input.kill_to_end(),
        KeyCode::Char('b') if ctrl && !alt => input.left(),
        KeyCode::Char('f') if ctrl && !alt => input.right(),
        KeyCode::Char(ch) if !ctrl && !alt => input.insert_char(ch),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left if ctrl || alt => input.word_left(),
        KeyCode::Right if ctrl || alt => input.word_right(),
        KeyCode::Left => input.left(),
        KeyCode::Right => input.right(),
        KeyCode::Up if alt => {
            input.history_prev();
        }
        KeyCode::Down if alt => {
            input.history_next();
        }
        KeyCode::Up => input.up(width),
        KeyCode::Down => input.down(width),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        _ => return false,
    }
    true
}

fn tool_mut<'a>(assistant: &'a mut AssistantMessage, id: &str) -> Option<&'a mut ToolPart> {
    assistant.content.iter_mut().find_map(|part| match part {
        AssistantPart::Tool(tool) if tool.id == id => Some(tool),
        _ => None,
    })
}

fn tool_input(state: &ToolState) -> Value {
    match state {
        ToolState::Streaming { input } => serde_json::from_str(input).unwrap_or_else(|_| json!({})),
        ToolState::Running { input, .. }
        | ToolState::Completed { input, .. }
        | ToolState::Error { input, .. } => input.clone(),
    }
}

fn set_text_part(assistant: &mut AssistantMessage, ordinal: usize, text: &str) {
    let position = assistant
        .content
        .iter()
        .enumerate()
        .filter(|(_, part)| matches!(part, AssistantPart::Text { .. }))
        .nth(ordinal)
        .map(|(index, _)| index);
    match position {
        Some(index) => {
            if let AssistantPart::Text { text: slot, .. } = &mut assistant.content[index] {
                *slot = text.to_string();
            }
        }
        None => assistant.content.push(AssistantPart::Text {
            text: text.to_string(),
            native: None,
        }),
    }
}

fn set_reasoning_part(assistant: &mut AssistantMessage, ordinal: usize, text: &str) {
    let position = assistant
        .content
        .iter()
        .enumerate()
        .filter(|(_, part)| matches!(part, AssistantPart::Reasoning { .. }))
        .nth(ordinal)
        .map(|(index, _)| index);
    match position {
        Some(index) => {
            if let AssistantPart::Reasoning { text: slot, .. } = &mut assistant.content[index] {
                *slot = text.to_string();
            }
        }
        None => assistant.content.push(AssistantPart::Reasoning {
            text: text.to_string(),
            native: None,
            time: None,
        }),
    }
}
