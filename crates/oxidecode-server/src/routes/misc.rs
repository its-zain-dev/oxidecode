//! Server info, pairing, location, configuration, project, VCS and worktree
//! endpoints.

use crate::error::{ApiError, ApiResult};
use crate::routes::{located, no_content, query_directory, string_param};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use oxidecode_core::ids;
use oxidecode_core::project as project_util;
use oxidecode_core::store::ProjectInfo;
use oxidecode_core::vcs;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub async fn info(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut urls = Vec::new();
    urls.push(format!("http://127.0.0.1:{}", state.port()));
    let hostname = state.hostname().to_string();
    if hostname != "localhost" && hostname != "127.0.0.1" {
        urls.push(format!("http://{}:{}", hostname, state.port()));
    }
    Ok(Json(json!({
        "version": state.version(),
        "pid": std::process::id(),
        "hostname": hostname,
        "urls": urls,
        "paths": { "tmp": state.paths().tmp.display().to_string() },
        "capabilities": { "persistentPty": true },
    })))
}

pub async fn pair(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let code = state.create_pair_token();
    Ok(Json(json!({ "code": code, "expires_in": 600 })))
}

pub async fn connect(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let browser = crate::routes::web::prefers_html(&headers);
    if state.consume_pair_token(&code) {
        // Pairing succeeds in two flavours: browsers get a cookie and are sent
        // to the dashboard, while API clients receive the session token that
        // they can use as a password (Basic or Bearer).
        let token = state.create_session_token();
        if browser {
            let mut response = Redirect::to("/").into_response();
            if let Ok(value) =
                HeaderValue::from_str(&crate::routes::web::session_cookie(&token))
            {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            return response;
        }
        return Json(json!({ "token": token })).into_response();
    }
    if browser {
        (
            StatusCode::NOT_FOUND,
            Html(
                "<!doctype html><meta charset=\"utf-8\"><title>OxideCode</title>\
                 <body style=\"background:#0a0a0a;color:#eeeeee;font-family:monospace;\
                 display:flex;align-items:center;justify-content:center;height:100vh\">\
                 <div><h1 style=\"color:#fab283\">OxideCode</h1>\
                 <p>This pairing code is invalid or has expired.</p>\
                 <p><a style=\"color:#8ab4ff\" href=\"/\">Return to the server</a></p>\
                 </div></body>"
                    .to_string(),
            ),
        )
            .into_response()
    } else {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "UnauthorizedError",
            "pairing code is invalid or has expired",
        )
        .into_response()
    }
}

pub async fn location(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let project = state.store().ensure_project(&directory).map_err(ApiError::from)?;
    Ok(Json(json!({
        "directory": directory.display().to_string(),
        "project": {
            "id": project.id,
            "directory": project.directory,
            "canonical": project.directory,
        }
    })))
}

pub async fn location_reload(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    state.invalidate_config(&directory);
    location(State(state), Query(params)).await
}

pub async fn config(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let loaded = state.config_for(&directory);
    let info = json!({
        "shell": loaded.config.shell.clone(),
        "model": loaded.config.model.clone(),
        "default_agent": loaded.config.default_agent.clone(),
        "permissions": loaded.config.permissions,
    });
    let mut entries: Vec<Value> = loaded
        .documents
        .iter()
        .map(|document| {
            json!({
                "type": "document",
                "path": document.path,
                "info": info,
            })
        })
        .collect();
    entries.push(json!({
        "type": "directory",
        "path": directory.display().to_string(),
    }));
    Ok(Json(json!(entries)))
}

pub async fn shells(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let _ = state;
    let mut options = Vec::new();
    for path in [
        "/system/bin/sh",
        "/bin/sh",
        "/bin/bash",
        "/usr/bin/bash",
        "/data/data/com.termux/files/usr/bin/bash",
        "/data/data/com.termux/files/usr/bin/zsh",
        "/data/data/com.termux/files/usr/bin/fish",
    ] {
        let file = PathBuf::from(path);
        if file.exists() {
            let name = file
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(path)
                .to_string();
            options.push(json!({ "path": path, "name": name, "acceptable": true }));
        }
    }
    Ok(Json(json!(options)))
}

fn project_json(project: &ProjectInfo) -> Value {
    json!({
        "id": project.id,
        "canonical": project.directory,
        "vcs": project.vcs,
        "name": project.name.clone().unwrap_or_else(|| project_util::display_name(std::path::Path::new(&project.directory))),
        "time": { "created": project.time.created, "updated": project.time.updated },
        "sandboxes": [],
    })
}

pub async fn projects(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let lists = state.store().list_projects().map_err(ApiError::from)?;
    let data: Vec<Value> = lists.iter().map(project_json).collect();
    Ok(Json(json!(data)))
}

pub async fn update_project(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let mut project = state
        .store()
        .get_project(&project_id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("project {project_id} not found")))?;
    if let Some(name) = body.get("name").and_then(Value::as_str) {
        project.name = Some(name.to_string());
    }
    if let Some(canonical) = body.get("canonical").and_then(Value::as_str) {
        project.directory = canonical.to_string();
    }
    project.time.updated = ids::now_ms();
    // Stored projects are read back from the sessions table; rewrite the row.
    let _ = state.store().ensure_project(std::path::Path::new(&project.directory));
    Ok(Json(project_json(&project)))
}

pub async fn vcs(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let branch = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || vcs::branch(&directory)
    })
    .await
    .unwrap_or_default();
    Ok(located(
        &directory.display().to_string(),
        json!({ "provider": "git", "branch": branch }),
    ))
}

pub async fn vcs_base(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let branch = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || vcs::branch(&directory)
    })
    .await
    .unwrap_or_default();
    let data = if vcs::is_git(&directory) {
        json!({ "name": branch.default, "ref": branch.default, "source": "default" })
    } else {
        Value::Null
    };
    Ok(located(&directory.display().to_string(), data))
}

pub async fn vcs_status(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let files = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || vcs::status_files(&directory)
    })
    .await
    .unwrap_or_default();
    Ok(located(&directory.display().to_string(), files))
}

pub async fn vcs_branches(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let search = string_param(&params, "search").map(|value| value.to_ascii_lowercase());
    let limit = string_param(&params, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200);
    let branches = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || vcs::branch_list(&directory)
    })
    .await
    .unwrap_or_default();
    let filtered: Vec<String> = branches
        .into_iter()
        .filter(|branch| {
            search
                .as_deref()
                .map(|needle| branch.to_ascii_lowercase().contains(needle))
                .unwrap_or(true)
        })
        .take(limit)
        .collect();
    Ok(located(&directory.display().to_string(), filtered))
}

pub async fn vcs_diff(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let mode = string_param(&params, "mode").unwrap_or("working").to_string();
    let diffs = tokio::task::spawn_blocking({
        let directory = directory.clone();
        move || vcs::diff(&directory, &mode)
    })
    .await
    .unwrap_or_default();
    Ok(located(&directory.display().to_string(), diffs))
}

pub async fn vcs_init(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let directory = query_directory(&state, &params);
    let _ = tokio::process::Command::new("git")
        .arg("init")
        .current_dir(&directory)
        .output()
        .await;
    Ok(no_content())
}

pub async fn worktrees(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let root = if vcs::is_git(&directory) {
        vcs::git_root(&directory).unwrap_or(directory.clone())
    } else {
        directory.clone()
    };
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let mut entries = Vec::new();
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.push(json!({ "directory": path }));
        }
    }
    if entries.is_empty() {
        entries.push(json!({ "directory": root.display().to_string() }));
    }
    Ok(located(&directory.display().to_string(), entries))
}

pub async fn worktree_create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let project = body
        .get("projectID")
        .and_then(Value::as_str)
        .and_then(|id| state.store().get_project(id).ok().flatten())
        .or_else(|| {
            body.get("from")
                .and_then(Value::as_str)
                .and_then(|from| state.store().ensure_project(std::path::Path::new(from)).ok())
        })
        .ok_or_else(|| ApiError::invalid("projectID or from is required"))?;
    let root = PathBuf::from(&project.directory);
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            body.get("branch")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("wt-{}", &ids::prefixed("")[..8]));
    let directory = body
        .get("directory")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| state.paths().worktrees_dir().join(&name));
    let branch = body
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_string();
    if let Some(parent) = directory.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(&root);
    command.args(["worktree", "add", "-B", &branch]);
    command.arg(&directory);
    if let Some(from) = body.get("from").and_then(Value::as_str) {
        command.arg(from);
    }
    let output = command.output().await.map_err(|error| ApiError::internal(error.to_string()))?;
    if !output.status.success() {
        return Err(ApiError::invalid(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(Json(json!({ "directory": directory.display().to_string() })))
}

pub async fn worktree_remove(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let directory = body
        .get("directory")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("directory is required"))?;
    let project = body
        .get("projectID")
        .and_then(Value::as_str)
        .and_then(|id| state.store().get_project(id).ok().flatten());
    let root = project
        .map(|project| PathBuf::from(project.directory))
        .unwrap_or_else(|| PathBuf::from(directory));
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(&root);
    command.args(["worktree", "remove", directory]);
    if body.get("force").and_then(Value::as_bool).unwrap_or(false) {
        command.arg("--force");
    }
    let _ = command.output().await;
    Ok(no_content())
}

pub async fn worktree_refresh(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    worktrees(State(state), Query(params)).await
}

pub async fn usage_limits(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let _ = state;
    Ok(Json(json!({ "data": [] })))
}

pub async fn generate_text(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _ = state;
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text: String = prompt.chars().take(60).collect();
    Ok(Json(json!({ "data": { "text": text } })))
}
