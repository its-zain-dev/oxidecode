//! OxideCode command line interface.
//!
//! The CLI is deliberately compatible with the commands the TermuxFileManager
//! Android app expects from OpenCode:
//!
//! ```text
//! opencode --version            -> 2.x.y  (major 2)
//! opencode service status       -> prints the HTTP URL
//! opencode service get password -> prints the Basic-auth password
//! opencode service start|restart|stop
//! opencode models               -> provider/model per line
//! opencode pair                 -> .../auth/connect/<code>
//! ```
//!
//! The TUI, one-shot runner and small session helpers are native Rust on top
//! of the same HTTP API, so the Android app and the terminal share one daemon.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use oxidecode_core::auth;
use oxidecode_core::config::LoadedConfig;
use oxidecode_core::ids;
use oxidecode_core::paths::Paths;
use oxidecode_core::util::atomic_write;
use oxidecode_provider::catalog;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// API compatibility version. OpenCode clients check the major version.
pub const COMPAT_VERSION: &str = "2.2.6";

#[derive(Parser, Debug)]
#[command(
    name = "oxidecode",
    version = COMPAT_VERSION,
    about = "OxideCode — a fast OpenCode-compatible coding agent",
    long_about = None,
    disable_help_subcommand = true,
    args_conflicts_with_subcommands = false
)]
struct Cli {
    /// Project directory (defaults to the current directory).
    #[arg(long, global = true, value_name = "DIR")]
    directory: Option<PathBuf>,
    /// Model to use, as `provider/model` or a model id.
    #[arg(long, global = true, value_name = "MODEL")]
    model: Option<String>,
    /// Agent to use (for example `build` or `plan`).
    #[arg(long, global = true, value_name = "AGENT")]
    agent: Option<String>,

    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Prompt to send immediately when the TUI opens.
    #[arg(value_name = "PROMPT", trailing_var_arg = true)]
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Open the terminal interface (default).
    Tui {
        /// Prompt to send immediately.
        #[arg(value_name = "PROMPT", trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Run the HTTP server in the foreground.
    Serve(ServeArgs),
    /// Send one prompt and print the response without opening the TUI.
    Run {
        /// Prompt text.
        #[arg(value_name = "PROMPT", required = true, trailing_var_arg = true)]
        prompt: Vec<String>,
        /// Print only the assistant text (default).
        #[arg(long)]
        json: bool,
    },
    /// List models as `provider/model`.
    Models {
        /// Include models whose provider is not configured.
        #[arg(long)]
        all: bool,
    },
    /// Inspect sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Manage the background service.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Print a pairing URL for the Android app.
    Pair,
    /// Developer helpers.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Args, Debug)]
struct ServeArgs {
    /// Bind address.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Bind port (0 selects a free port).
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Override the service password (defaults to the stored password).
    #[arg(long)]
    password: Option<String>,
    /// Directory the server defaults to.
    #[arg(long)]
    directory: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum SessionCommand {
    /// List recent sessions.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Print a session transcript.
    Show {
        /// Session ID.
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Delete a session.
    Delete {
        /// Session ID.
        id: String,
    },
    /// Create an empty session.
    New {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceCommand {
    /// Print the service URL when the daemon is running.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Start the daemon (idempotent).
    Start,
    /// Restart the daemon.
    Restart,
    /// Stop the daemon.
    Stop,
    /// Read a service value.
    Get {
        #[arg(value_enum)]
        field: ServiceField,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum ServiceField {
    Password,
    Url,
}

#[derive(Subcommand, Debug)]
enum DebugCommand {
    /// Print the resolved application paths.
    Paths,
    /// Print the service metadata file.
    Service,
}

pub fn run() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let paths = Paths::discover();
    paths.ensure().context("creating data directories")?;
    let directory = match &cli.directory {
        Some(directory) => directory
            .canonicalize()
            .unwrap_or_else(|_| directory.clone()),
        None => std::env::current_dir().context("reading the current directory")?,
    };

    match cli.command {
        Some(CliCommand::Tui { prompt }) => {
            let prompt = if prompt.is_empty() {
                None
            } else {
                Some(prompt.join(" "))
            };
            open_tui(&paths, &directory, prompt, cli.model, cli.agent)
        }
        Some(CliCommand::Serve(args)) => serve(args, &paths, &directory),
        Some(CliCommand::Run { prompt, json }) => {
            run_once(&paths, &directory, &prompt.join(" "), cli.model, cli.agent, json)
        }
        Some(CliCommand::Models { all }) => list_models(&paths, &directory, all),
        Some(CliCommand::Session { command }) => session_command(&paths, &directory, command),
        Some(CliCommand::Service { command }) => service_command(&paths, &directory, command),
        Some(CliCommand::Pair) => pair(&paths, &directory),
        Some(CliCommand::Debug { command }) => debug_command(&paths, command),
        None => {
            let prompt = if cli.prompt.is_empty() {
                None
            } else {
                Some(cli.prompt.join(" "))
            };
            open_tui(&paths, &directory, prompt, cli.model, cli.agent)
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("OXIDECODE_LOG")
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn open_tui(
    paths: &Paths,
    directory: &Path,
    prompt: Option<String>,
    model: Option<String>,
    agent: Option<String>,
) -> Result<()> {
    let endpoint = ensure_service(paths, directory)?;
    let options = oxidecode_tui::Options {
        base: endpoint.url.clone(),
        password: endpoint.password.clone(),
        directory: directory.display().to_string(),
        prompt,
        model,
        agent,
    };
    oxidecode_tui::run(options)
}

fn list_models(paths: &Paths, directory: &Path, all: bool) -> Result<()> {
    let config = LoadedConfig::load(paths, directory);
    let mut models: Vec<_> = catalog::models(&config)
        .into_iter()
        .filter(|model| all || model.enabled)
        .collect();
    models.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
    for model in models {
        println!("{}/{}", model.provider_id, model.id);
    }
    Ok(())
}

fn session_command(paths: &Paths, directory: &Path, command: SessionCommand) -> Result<()> {
    let endpoint = ensure_service(paths, directory)?;
    let api = Api::new(&endpoint.url, &endpoint.password);
    let location = directory.display().to_string();
    match command {
        SessionCommand::List { json, limit } => {
            let value = api.get_dir(
                &format!("/api/session?limit={limit}&order=desc"),
                &location,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
                return Ok(());
            }
            if let Some(data) = value.get("data").and_then(Value::as_array) {
                for session in data {
                    let id = session.get("id").and_then(Value::as_str).unwrap_or("");
                    let title = session.get("title").and_then(Value::as_str).unwrap_or("");
                    println!("{id}\t{title}");
                }
            }
            Ok(())
        }
        SessionCommand::Show { id, json } => {
            let value = api.get_dir(
                &format!("/api/session/{id}/message?limit=5000&order=asc"),
                &location,
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
                return Ok(());
            }
            if let Some(data) = value.get("data").and_then(Value::as_array) {
                for message in data {
                    print_message(message);
                }
            }
            Ok(())
        }
        SessionCommand::Delete { id } => {
            api.delete_dir(&format!("/api/session/{id}"), &location)?;
            println!("deleted {id}");
            Ok(())
        }
        SessionCommand::New { title, json } => {
            let mut body = json!({ "location": { "directory": location } });
            if let Some(title) = title {
                body["title"] = json!(title);
            }
            let value = api.post_dir("/api/session", body, &location)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else if let Some(id) = value
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(Value::as_str)
            {
                println!("{id}");
            }
            Ok(())
        }
    }
}

fn print_message(message: &Value) {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "user" => {
            println!("User:");
            println!(
                "{}\n",
                message.get("text").and_then(Value::as_str).unwrap_or("")
            );
        }
        "assistant" => {
            if let Some(parts) = message.get("content").and_then(Value::as_array) {
                for part in parts {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                            if !text.trim().is_empty() {
                                println!("{text}\n");
                            }
                        }
                        Some("tool") => {
                            let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                            println!("[tool] {name}");
                        }
                        _ => {}
                    }
                }
            }
            if let Some(error) = message.get("error") {
                if let Some(text) = error.get("message").and_then(Value::as_str) {
                    eprintln!("error: {text}");
                }
            }
        }
        "shell" => {
            println!(
                "$ {}",
                message.get("command").and_then(Value::as_str).unwrap_or("")
            );
            if let Some(output) = message.get("output") {
                if let Some(text) = output.get("text").and_then(Value::as_str) {
                    println!("{text}");
                }
            }
            println!();
        }
        "idle" => {
            if message.get("outcome").and_then(Value::as_str) == Some("failed") {
                if let Some(text) = message
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                {
                    eprintln!("error: {text}");
                }
            }
        }
        "system" | "synthetic" | "skill" => {
            if let Some(text) = message.get("text").and_then(Value::as_str) {
                println!("{text}\n");
            }
        }
        _ => {}
    }
}

fn run_once(
    paths: &Paths,
    directory: &Path,
    prompt: &str,
    model: Option<String>,
    agent: Option<String>,
    json_output: bool,
) -> Result<()> {
    if prompt.trim().is_empty() {
        bail!("empty prompt");
    }
    let endpoint = ensure_service(paths, directory)?;
    let api = Api::new(&endpoint.url, &endpoint.password);
    let location = directory.display().to_string();

    let mut body = json!({ "location": { "directory": location } });
    if let Some(model) = model.as_deref().and_then(|value| model_json(paths, directory, value)) {
        body["model"] = model;
    }
    if let Some(agent) = agent.filter(|value| !value.is_empty()) {
        body["agent"] = json!(agent);
    }
    let title: String = prompt.lines().next().unwrap_or("Session").chars().take(80).collect();
    body["title"] = json!(title);
    let created = api.post_no_dir("/api/session", body.clone())?;
    let session_id = created
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(Value::as_str)
        .context("session creation did not return an id")?
        .to_string();
    api.post_dir(
        &format!("/api/session/{session_id}/prompt"),
        json!({ "text": prompt }),
        &location,
    )?;

    let mut printed: HashMap<String, String> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut saw_assistant = false;
    let mut idle_ticks = 0u32;
    let started = Instant::now();
    let deadline = Duration::from_secs(20 * 60);

    loop {
        std::thread::sleep(Duration::from_millis(150));
        let messages = api.get_dir(
            &format!("/api/session/{session_id}/message?limit=5000&order=asc"),
            &location,
        )?;
        if let Some(data) = messages.get("data").and_then(Value::as_array) {
            for message in data {
                if message.get("type").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let text = assistant_text(message);
                if !text.is_empty() {
                    saw_assistant = true;
                    let previous = printed.get(&id).cloned().unwrap_or_default();
                    if text.len() > previous.len() && text.starts_with(&previous) {
                        let delta = &text[previous.len()..];
                        if !json_output {
                            print!("{delta}");
                            let _ = std::io::stdout().flush();
                        }
                    } else if text != previous {
                        if !json_output {
                            println!("{text}");
                        }
                    }
                    printed.insert(id, text);
                }
                if let Some(error) = message
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                {
                    errors.push(error.to_string());
                }
            }
        }

        let active = api.get_dir("/api/session/active", &location)?;
        let running = active
            .get("data")
            .and_then(Value::as_object)
            .map(|sessions| sessions.contains_key(&session_id))
            .unwrap_or(false);
        if running {
            idle_ticks = 0;
        } else if started.elapsed() > Duration::from_millis(700)
            && (saw_assistant || started.elapsed() > Duration::from_secs(2))
        {
            idle_ticks += 1;
            if idle_ticks >= 3 {
                break;
            }
        }
        if started.elapsed() > deadline {
            bail!("timed out waiting for the model");
        }
    }

    if !json_output {
        println!();
    }
    if let Some(error) = errors.last() {
        bail!("{error}");
    }
    Ok(())
}

fn assistant_text(message: &Value) -> String {
    let mut out = String::new();
    if let Some(parts) = message.get("content").and_then(Value::as_array) {
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

fn model_json(paths: &Paths, directory: &Path, value: &str) -> Option<Value> {
    if let Some((provider, rest)) = value.split_once('/') {
        let (id, variant) = match rest.split_once('#') {
            Some((id, variant)) if !variant.is_empty() => (id, Some(variant)),
            _ => (rest, None),
        };
        let mut model = json!({ "providerID": provider, "id": id });
        if let Some(variant) = variant {
            model["variant"] = json!(variant);
        }
        return Some(model);
    }
    let config = LoadedConfig::load(paths, directory);
    let models = catalog::models(&config);
    let found = models
        .iter()
        .find(|model| model.enabled && model.model_id == value)
        .or_else(|| models.iter().find(|model| model.enabled && model.id == value))
        .or_else(|| models.iter().find(|model| model.model_id == value))?;
    Some(json!({ "providerID": found.provider_id, "id": found.id }))
}

fn pair(paths: &Paths, directory: &Path) -> Result<()> {
    let endpoint = ensure_service(paths, directory)?;
    let api = Api::new(&endpoint.url, &endpoint.password);
    let value = api.post_no_dir("/api/pair", json!({}))?;
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .context("the server did not return a pairing code")?;
    println!("{}/auth/connect/{}", endpoint.url.trim_end_matches('/'), code);
    Ok(())
}

fn debug_command(paths: &Paths, command: DebugCommand) -> Result<()> {
    match command {
        DebugCommand::Paths => {
            let value = json!({
                "home": paths.home,
                "config": paths.config,
                "data": paths.data,
                "state": paths.state,
                "cache": paths.cache,
                "tmp": paths.tmp,
                "database": paths.database(),
                "serviceFile": paths.service_file(),
                "logFile": paths.log_file(),
                "password": paths.service_file(),
                "serviceInfo": service_info_path(paths),
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        DebugCommand::Service => {
            let path = service_info_path(paths);
            match fs::read_to_string(&path) {
                Ok(text) => {
                    println!("{text}");
                    Ok(())
                }
                Err(error) => bail!("{}: {error}", path.display()),
            }
        }
    }
}

fn serve(args: ServeArgs, paths: &Paths, directory: &Path) -> Result<()> {
    let host: IpAddr = args
        .host
        .parse()
        .or_else(|_| match args.host.as_str() {
            "localhost" => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            other => bail!("invalid host `{other}`"),
        })?;
    let directory = args
        .directory
        .unwrap_or_else(|| directory.to_path_buf())
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    if args.password.is_none() {
        // Make sure the password exists before the server reads it.
        auth::load_or_create_password(&paths.service_file())?;
    }
    let options = oxidecode_server::ServerOptions {
        host,
        port: args.port,
        directory,
        password: args.password,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creating the async runtime")?;
    runtime.block_on(async move {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(oxidecode_server::serve(options, Some(sender)));
        match receiver.await {
            Ok(address) => {
                println!("http://{address}");
                let _ = std::io::stdout().flush();
            }
            Err(_) => {
                task.await??;
                bail!("server stopped before binding");
            }
        }
        task.await??;
        Ok(())
    })
}

struct Endpoint {
    url: String,
    password: String,
}

struct Api {
    base: String,
    password: String,
    client: reqwest::blocking::Client,
}

impl Api {
    fn new(base: &str, password: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            base: base.trim_end_matches('/').to_string(),
            password: password.to_string(),
            client,
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, format!("{}{}", self.base, path))
            .basic_auth("opencode", Some(&self.password))
            .header(reqwest::header::ACCEPT, "application/json")
    }

    fn get_dir(&self, path: &str, directory: &str) -> Result<Value> {
        let response = self
            .request(reqwest::Method::GET, path)
            .query(&[("location[directory]", directory)])
            .send()?;
        decode(response)
    }

    fn delete_dir(&self, path: &str, directory: &str) -> Result<Value> {
        let response = self
            .request(reqwest::Method::DELETE, path)
            .query(&[("location[directory]", directory)])
            .send()?;
        decode(response)
    }

    fn post_dir(&self, path: &str, body: Value, directory: &str) -> Result<Value> {
        let response = self
            .request(reqwest::Method::POST, path)
            .query(&[("location[directory]", directory)])
            .json(&body)
            .send()?;
        decode(response)
    }

    fn post_no_dir(&self, path: &str, body: Value) -> Result<Value> {
        let response = self
            .request(reqwest::Method::POST, path)
            .json(&body)
            .send()?;
        decode(response)
    }
}

fn decode(response: reqwest::blocking::Response) -> Result<Value> {
    let status = response.status();
    let bytes = response.bytes()?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .or_else(|| value.get("error"))
                    .and_then(|error| {
                        error
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| error.get("message").and_then(Value::as_str).map(str::to_string))
                    })
            })
            .unwrap_or_else(|| {
                if text.is_empty() {
                    status.to_string()
                } else {
                    text.to_string()
                }
            });
        bail!("{message}");
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn service_command(paths: &Paths, directory: &Path, command: ServiceCommand) -> Result<()> {
    match command {
        ServiceCommand::Status { json } => {
            let info = running_info(paths).context("service is not running")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else if let Some(url) = info.get("url").and_then(Value::as_str) {
                println!("{url}");
            }
            Ok(())
        }
        ServiceCommand::Start => {
            let endpoint = ensure_service(paths, directory)?;
            println!("{}", endpoint.url);
            Ok(())
        }
        ServiceCommand::Restart => {
            let _ = stop_service(paths);
            let endpoint = start_service(paths, directory)?;
            println!("{}", endpoint.url);
            Ok(())
        }
        ServiceCommand::Stop => {
            stop_service(paths)?;
            eprintln!("stopped");
            Ok(())
        }
        ServiceCommand::Get { field } => match field {
            ServiceField::Password => {
                println!("{}", service_password(paths)?);
                Ok(())
            }
            ServiceField::Url => {
                let info = running_info(paths).context("service is not running")?;
                if let Some(url) = info.get("url").and_then(Value::as_str) {
                    println!("{url}");
                }
                Ok(())
            }
        },
    }
}

fn ensure_service(paths: &Paths, directory: &Path) -> Result<Endpoint> {
    let password = service_password(paths)?;
    if let Some(info) = read_info(paths) {
        let url = info.get("url").and_then(Value::as_str).map(str::to_string);
        let pid = info.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
        if let Some(url) = url {
            if probe(&url, &password) {
                return Ok(Endpoint { url, password });
            }
            if pid > 1 {
                let _ = terminate(pid);
            }
        }
        let _ = fs::remove_file(service_info_path(paths));
    }
    start_service_with_password(paths, directory, password)
}

fn start_service(paths: &Paths, directory: &Path) -> Result<Endpoint> {
    let password = service_password(paths)?;
    start_service_with_password(paths, directory, password)
}

fn start_service_with_password(
    paths: &Paths,
    directory: &Path,
    password: String,
) -> Result<Endpoint> {
    let port = match std::env::var("OXIDECODE_PORT")
        .or_else(|_| std::env::var("OPENCODE_PORT"))
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
    {
        Some(port) => port,
        None => free_port()?,
    };
    let executable = std::env::current_exe().context("locating the OxideCode binary")?;
    let log_path = paths.log_file();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening the log {}", log_path.display()))?;
    let log_clone = log.try_clone()?;

    let mut command = Command::new(&executable);
    command
        .arg("serve")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--directory")
        .arg(directory)
        .env("OXIDECODE_SERVICE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_clone))
        .stderr(Stdio::from(log));
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", executable.display()))?;
    let pid = child.id();
    let url = format!("http://127.0.0.1:{port}");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if probe(&url, &password) {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                bail!(
                    "server exited with {status}\n{}",
                    log_tail(&log_path).unwrap_or_default()
                );
            }
            Ok(None) => {}
            Err(error) => bail!("waiting for the server: {error}"),
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            bail!(
                "timed out waiting for the server\n{}",
                log_tail(&log_path).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(child);

    let info = json!({
        "pid": pid,
        "port": port,
        "url": url,
        "directory": directory.display().to_string(),
        "started": ids::now_ms(),
        "version": COMPAT_VERSION,
    });
    atomic_write(
        &service_info_path(paths),
        serde_json::to_string_pretty(&info)?.as_bytes(),
    )?;
    Ok(Endpoint { url, password })
}

fn stop_service(paths: &Paths) -> Result<()> {
    let Some(info) = read_info(paths) else {
        bail!("service is not running");
    };
    if let Some(pid) = info.get("pid").and_then(Value::as_u64) {
        terminate(pid as u32)?;
    }
    let _ = fs::remove_file(service_info_path(paths));
    Ok(())
}

fn running_info(paths: &Paths) -> Option<Value> {
    let info = read_info(paths)?;
    let password = auth::load_or_create_password(&paths.service_file()).ok()?;
    let url = info.get("url").and_then(Value::as_str)?;
    if probe(url, &password) {
        Some(info)
    } else {
        let _ = fs::remove_file(service_info_path(paths));
        None
    }
}

fn probe(url: &str, password: &str) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(900))
        .connect_timeout(Duration::from_millis(400))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client
        .get(format!("{}/api/model/default", url.trim_end_matches('/')))
        .basic_auth("opencode", Some(password))
        .send()
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

fn terminate(pid: u32) -> Result<()> {
    if pid <= 1 {
        return Ok(());
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    for _ in 0..50 {
        if !alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    for _ in 0..20 {
        if !alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .context("finding a free port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn service_password(paths: &Paths) -> Result<String> {
    auth::load_or_create_password(&paths.service_file()).context("loading the service password")
}

fn service_info_path(paths: &Paths) -> PathBuf {
    paths.state.join("service.json")
}

fn read_info(paths: &Paths) -> Option<Value> {
    let text = fs::read_to_string(service_info_path(paths)).ok()?;
    serde_json::from_str(&text).ok()
}

fn log_tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(4000);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buffer = String::new();
    let _ = file.read_to_string(&mut buffer);
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Formats a child process failure with the tail of a log file.
#[allow(dead_code)]
fn child_failure(child: &mut Child) -> String {
    match child.try_wait() {
        Ok(Some(status)) => status.to_string(),
        Ok(None) => "still running".to_string(),
        Err(error) => error.to_string(),
    }
}
