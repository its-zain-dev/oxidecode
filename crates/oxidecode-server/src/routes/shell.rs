//! Background shell endpoints (`/api/shell`).
//!
//! Unlike the agent `bash` tool these jobs are not scoped to a session: the CLI
//! and TUI use them for long running commands whose output is polled.

use crate::error::{ApiError, ApiResult};
use crate::routes::{located, no_content, query_directory};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use oxidecode_core::ids;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::AsyncReadExt;

struct Entry {
    id: String,
    status: String,
    command: String,
    cwd: String,
    shell: String,
    file: String,
    pid: u32,
    exit: Option<i32>,
    signal: Option<String>,
    metadata: Value,
    started: i64,
    completed: Option<i64>,
    output: Arc<Mutex<String>>,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_json(entry: &Entry) -> Value {
    json!({
        "id": entry.id,
        "status": entry.status,
        "command": entry.command,
        "cwd": entry.cwd,
        "shell": entry.shell,
        "file": entry.file,
        "pid": entry.pid,
        "exit": entry.exit,
        "signal": entry.signal,
        "metadata": entry.metadata,
        "time": {
            "started": entry.started,
            "completed": entry.completed,
        },
    })
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
        .ok_or_else(|| ApiError::invalid("command is required"))?
        .to_string();
    let cwd = body
        .get("cwd")
        .and_then(Value::as_str)
        .map(|value| {
            let path = std::path::PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                directory.join(path)
            }
        })
        .unwrap_or_else(|| directory.clone());
    let config = state.config_for(&directory);
    let shell = config
        .config
        .shell
        .clone()
        .unwrap_or_else(|| "/system/bin/sh".to_string());
    let timeout = body.get("timeout").and_then(Value::as_u64);

    let mut builder = tokio::process::Command::new(&shell);
    builder
        .arg("-c")
        .arg(&command)
        .current_dir(&cwd)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = builder
        .spawn()
        .map_err(|error| ApiError::internal(format!("spawn: {error}")))?;
    let pid = child.id().unwrap_or_default();
    let id = ids::prefixed("shl");
    let started = ids::now_ms();
    let output = Arc::new(Mutex::new(String::new()));
    let status = Arc::new(Mutex::new("running".to_string()));
    let exit = Arc::new(Mutex::new(None::<i32>));
    let entry = Entry {
        id: id.clone(),
        status: "running".to_string(),
        command,
        cwd: cwd.display().to_string(),
        shell,
        file: String::new(),
        pid,
        exit: None,
        signal: None,
        metadata: body
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| json!({})),
        started,
        completed: None,
        output: output.clone(),
    };
    let value = entry_json(&entry);
    if let Ok(mut registry) = registry().lock() {
        registry.insert(id.clone(), entry);
    }

    let child = Arc::new(tokio::sync::Mutex::new(child));
    let child_handle = child.clone();
    let output_handle = output.clone();
    let status_handle = status.clone();
    let exit_handle = exit.clone();
    let _context_directory = directory.display().to_string();
    tokio::spawn(async move {
        let mut child = child_handle.lock().await;
        if let Some(mut stdout) = child.stdout.take() {
            let output = output_handle.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                loop {
                    match stdout.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut output) = output.lock() {
                                output.push_str(&String::from_utf8_lossy(&buffer[..read]));
                            }
                        }
                    }
                }
            });
        }
        if let Some(mut stderr) = child.stderr.take() {
            let output = output_handle.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                loop {
                    match stderr.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut output) = output.lock() {
                                output.push_str(&String::from_utf8_lossy(&buffer[..read]));
                            }
                        }
                    }
                }
            });
        }
        let result = if let Some(timeout) = timeout {
            match tokio::time::timeout(
                std::time::Duration::from_millis(timeout.max(1)),
                child.wait(),
            )
            .await
            {
                Ok(status) => status.ok(),
                Err(_) => {
                    let _ = child.kill().await;
                    *status_handle.lock().unwrap() = "timeout".to_string();
                    None
                }
            }
        } else {
            child.wait().await.ok()
        };
        let completed = ids::now_ms();
        let code = result.and_then(|status| status.code());
        *exit_handle.lock().unwrap() = code;
        {
            let mut status = status_handle.lock().unwrap();
            if *status != "timeout" {
                *status = "exited".to_string();
            }
        }
        if let Ok(mut registry) = registry().lock() {
            if let Some(entry) = registry.get_mut(&id) {
                entry.status = status_handle.lock().unwrap().clone();
                entry.exit = code;
                entry.completed = Some(completed);
            }
        }
    });

    Ok(located(&directory.display().to_string(), value))
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

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let registry = registry()
        .lock()
        .map_err(|_| ApiError::internal("shell registry poisoned"))?;
    let entry = registry
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("shell {id} not found")))?;
    Ok(located(&directory.display().to_string(), entry_json(entry)))
}

pub async fn output(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let cursor = params
        .get("cursor")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(64 * 1024);
    let registry = registry()
        .lock()
        .map_err(|_| ApiError::internal("shell registry poisoned"))?;
    let entry = registry
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("shell {id} not found")))?;
    let output = entry.output.lock().map(|value| value.clone()).unwrap_or_default();
    let size = output.len();
    let start = cursor.min(size);
    let end = (start + limit).min(size);
    let slice = output
        .get(start..end)
        .unwrap_or_default()
        .to_string();
    Ok(located(
        &directory.display().to_string(),
        json!({
            "output": slice,
            "cursor": end,
            "size": size,
            "truncated": end < size,
        }),
    ))
}

pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let _ = state;
    if let Ok(mut registry) = registry().lock() {
        registry.remove(&id);
    }
    Ok(no_content())
}
