//! PTY endpoints backed by the C runtime (`C/ocpty.c`).
//!
//! The terminal itself is rendered by the client; the server only owns the
//! pseudo-terminal, its scrollback buffer and the resize/terminate operations.

use crate::error::{ApiError, ApiResult};
use crate::routes::{located, no_content, query_directory};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use oxidecode_sys::Pty;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const MAX_BUFFER: usize = 2 * 1024 * 1024;

struct Entry {
    id: String,
    title: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    status: String,
    pid: i32,
    exit_code: Option<i32>,
    pty: Option<Pty>,
    buffer: Arc<Mutex<VecDeque<u8>>>,
    token: Option<String>,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_json(entry: &Entry) -> Value {
    json!({
        "id": entry.id,
        "title": entry.title,
        "command": entry.command,
        "args": entry.args,
        "cwd": entry.cwd,
        "status": entry.status,
        "pid": entry.pid.max(0) as u64,
        "exitCode": entry.exit_code.unwrap_or(0).max(0) as u64,
    })
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let entries: Vec<Value> = registry()
        .lock()
        .map(|registry| registry.values().map(entry_json).collect())
        .unwrap_or_default();
    Ok(located(&directory.display().to_string(), entries))
}

pub async fn create(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let command = body
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::env::var("SHELL").unwrap_or_else(|_| "/system/bin/sh".to_string())
        });
    let args: Vec<String> = body
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let cwd = body
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                directory.join(path)
            }
        })
        .unwrap_or_else(|| directory.clone());
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Terminal")
        .to_string();
    let rows = 24u16;
    let cols = 80u16;

    let mut argv: Vec<OsString> = Vec::new();
    argv.push(OsString::from(&command));
    for arg in &args {
        argv.push(OsString::from(arg));
    }
    let argv_refs: Vec<&std::ffi::OsStr> = argv.iter().map(OsString::as_os_str).collect();
    let mut env: Vec<OsString> = std::env::vars_os()
        .map(|(key, value)| {
            let mut entry = key;
            entry.push("=");
            entry.push(value);
            entry
        })
        .collect();
    if let Some(extra) = body.get("env").and_then(Value::as_object) {
        for (key, value) in extra {
            if let Some(value) = value.as_str() {
                let mut entry = OsString::from(key);
                entry.push("=");
                entry.push(value);
                env.push(entry);
            }
        }
    }
    env.push(OsString::from("TERM=xterm-256color"));
    env.push(OsString::from("COLORTERM=truecolor"));
    let env_refs: Vec<&std::ffi::OsStr> = env.iter().map(OsString::as_os_str).collect();

    let pty = Pty::spawn(Some(&cwd), &argv_refs, &env_refs, rows, cols)        .map_err(|error| ApiError::internal(format!("pty: {error}")))?;
    let pid = pty.pid();
    let buffer = Arc::new(Mutex::new(VecDeque::new()));
    if let Ok(mut master) = pty.master().try_clone() {
        let buffer = buffer.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match master.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if let Ok(mut buffer) = buffer.lock() {
                            for byte in &chunk[..read] {
                                if buffer.len() >= MAX_BUFFER {
                                    buffer.pop_front();
                                }
                                buffer.push_back(*byte);
                            }
                        }
                    }
                }
            }
        });
    }

    let id = oxidecode_core::ids::prefixed("pty");
    let entry = Entry {
        id: id.clone(),
        title,
        command,
        args,
        cwd: cwd.display().to_string(),
        status: "running".to_string(),
        pid,
        exit_code: None,
        pty: Some(pty),
        buffer,
        token: None,
    };
    let value = entry_json(&entry);
    if let Ok(mut registry) = registry().lock() {
        registry.insert(id.clone(), entry);
    }
    let _ = state;
    Ok(located(&cwd.display().to_string(), value))
}

pub async fn get(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let mut registry = registry()
        .lock()
        .map_err(|_| ApiError::internal("pty registry poisoned"))?;
    let entry = registry
        .get_mut(&pty_id)
        .ok_or_else(|| ApiError::not_found(format!("pty {pty_id} not found")))?;
    if let Some(pty) = entry.pty.as_mut() {
        if let Ok(Some(status)) = pty.try_wait() {
            entry.status = "exited".to_string();
            entry.exit_code = Some(status);
        }
    }
    Ok(located(&directory.display().to_string(), entry_json(entry)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let mut registry = registry()
        .lock()
        .map_err(|_| ApiError::internal("pty registry poisoned"))?;
    let entry = registry
        .get_mut(&pty_id)
        .ok_or_else(|| ApiError::not_found(format!("pty {pty_id} not found")))?;
    if let Some(title) = body.get("title").and_then(Value::as_str) {
        entry.title = title.to_string();
    }
    if let Some(size) = body.get("size") {
        let rows = size.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
        let cols = size.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
        if let Some(pty) = entry.pty.as_ref() {
            let _ = pty.resize(rows, cols);
        }
    }
    Ok(located(&directory.display().to_string(), entry_json(entry)))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let directory = query_directory(&state, &params);
    if let Ok(mut registry) = registry().lock() {
        if let Some(mut entry) = registry.remove(&pty_id) {
            if let Some(pty) = entry.pty.as_mut() {
                let _ = pty.terminate();
            }
        }
    }
    let _ = directory;
    Ok(no_content())
}

pub async fn connect_token(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
) -> ApiResult<Json<Value>> {
    use rand::Rng;
    let mut registry = registry()
        .lock()
        .map_err(|_| ApiError::internal("pty registry poisoned"))?;
    let entry = registry
        .get_mut(&pty_id)
        .ok_or_else(|| ApiError::not_found(format!("pty {pty_id} not found")))?;
    let mut rng = rand::thread_rng();
    let token: String = (0..32)
        .map(|_| {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            ALPHABET[rng.gen_range(0..ALPHABET.len())] as char
        })
        .collect();
    entry.token = Some(token.clone());
    let _ = state;
    Ok(Json(json!({
        "token": token,
        "url": format!("/api/pty/{pty_id}/connect?token={token}"),
    })))
}

/// Returns the raw scrollback buffer (used by clients that do not speak the
/// WebSocket protocol).
pub fn snapshot(pty_id: &str) -> Option<Vec<u8>> {
    let registry = registry().lock().ok()?;
    let entry = registry.get(pty_id)?;
    let buffer = entry.buffer.lock().ok()?;
    Some(buffer.iter().copied().collect())
}
