//! HTTP routing, authentication and the shared JSON helpers used by every
//! endpoint group.

pub mod catalog;
pub mod event;
pub mod fs;
pub mod misc;
pub mod permission;
pub mod pty;
pub mod session;
pub mod shell;

use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use oxidecode_core::auth;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

/// `{"data": ...}` is the envelope used by session, model and provider routes.
pub fn data<T: serde::Serialize>(value: T) -> Json<Value> {
    Json(json!({ "data": value }))
}

/// `{"location": {...}, "data": ...}` is used by location scoped routes.
pub fn located<T: serde::Serialize>(directory: &str, value: T) -> Json<Value> {
    Json(json!({
        "location": { "directory": directory },
        "data": value,
    }))
}

/// Reads the `location[directory]` query parameter, also accepting a plain
/// `directory` for convenience clients.
pub fn query_directory(
    state: &AppState,
    params: &HashMap<String, String>,
) -> PathBuf {
    let requested = params
        .get("location[directory]")
        .or_else(|| params.get("location"))
        .or_else(|| params.get("directory"))
        .map(String::as_str);
    state.location(requested)
}

pub fn string_param<'a>(
    params: &'a HashMap<String, String>,
    key: &str,
) -> Option<&'a str> {
    params.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/info", get(misc::info))
        .route("/api/pair", post(misc::pair))
        .route("/api/location", get(misc::location))
        .route("/api/location/reload", post(misc::location_reload))
        .route("/api/config", get(misc::config))
        .route("/api/config/shell", get(misc::shells))
        .route("/api/project", get(misc::projects))
        .route("/api/project/{projectID}", patch(misc::update_project))
        .route("/api/vcs", get(misc::vcs))
        .route("/api/vcs/base", get(misc::vcs_base))
        .route("/api/vcs/status", get(misc::vcs_status))
        .route("/api/vcs/branch", get(misc::vcs_branches))
        .route("/api/vcs/diff", get(misc::vcs_diff))
        .route("/api/vcs/init", post(misc::vcs_init))
        .route("/api/worktree", get(misc::worktrees).post(misc::worktree_create).delete(misc::worktree_remove))
        .route("/api/worktree/refresh", post(misc::worktree_refresh))
        .route("/api/agent", get(catalog::agents))
        .route("/api/agent/{agentID}", get(catalog::agent))
        .route("/api/model", get(catalog::models))
        .route("/api/model/default", get(catalog::default_model))
        .route("/api/provider", get(catalog::providers))
        .route("/api/provider/{providerID}", get(catalog::provider))
        .route("/api/command", get(catalog::commands))
        .route("/api/skill", get(catalog::skills))
        .route("/api/plugin", get(catalog::empty_list))
        .route("/api/mcp", get(catalog::mcp))
        .route("/api/integration", get(catalog::integrations))
        .route("/api/credential", get(catalog::credentials))
        .route("/api/reference", get(catalog::references))
        .route("/api/websearch/provider", get(catalog::websearch_providers))
        .route("/api/permission/request", get(permission::list_all))
        .route("/api/permission/saved", get(permission::saved))
        .route("/api/permission/saved/{id}", patch(permission::update_saved).delete(permission::remove_saved))
        .route("/api/session", get(session::list).post(session::create))
        .route("/api/session/active", get(session::active))
        .route("/api/session/{sessionID}", get(session::get).patch(session::update).delete(session::remove))
        .route("/api/session/{sessionID}/fork", post(session::fork))
        .route("/api/session/{sessionID}/agent", post(session::switch_agent))
        .route("/api/session/{sessionID}/model", post(session::switch_model))
        .route("/api/session/{sessionID}/move", post(session::move_session))
        .route("/api/session/{sessionID}/prompt", post(session::prompt))
        .route("/api/session/{sessionID}/command", post(session::command))
        .route("/api/session/{sessionID}/synthetic", post(session::synthetic))
        .route("/api/session/{sessionID}/shell", post(session::shell))
        .route("/api/session/{sessionID}/compact", post(session::compact))
        .route("/api/session/{sessionID}/generate", post(session::generate))
        .route("/api/session/{sessionID}/interrupt", post(session::interrupt))
        .route("/api/session/{sessionID}/message", get(session::messages))
        .route("/api/session/{sessionID}/message/{messageID}", get(session::message))
        .route("/api/session/{sessionID}/permission", get(permission::list).post(permission::create))
        .route("/api/session/{sessionID}/permission/{requestID}", get(permission::get))
        .route("/api/session/{sessionID}/permission/{requestID}/reply", post(permission::reply))
        .route("/api/session/{sessionID}/context", get(session::context))
        .route("/api/session/{sessionID}/diff", get(session::diff))
        .route("/api/session/{sessionID}/inbox", get(session::inbox))
        .route("/api/session/{sessionID}/inbox/{inboxID}", patch(session::update_inbox).delete(session::cancel_inbox))
        .route("/api/session/{sessionID}/revert/stage", post(session::revert_stage))
        .route("/api/session/{sessionID}/revert/commit", post(session::revert_commit))
        .route("/api/session/{sessionID}/revert", delete(session::revert_clear))
        .route("/api/session/{sessionID}/view", post(session::view))
        .route("/api/session/{sessionID}/export", get(session::export))
        .route("/api/experimental/session/import", post(session::import))
        .route("/api/experimental/session/stats", get(session::stats))
        .route("/api/experimental/session/{sessionID}/export", get(session::export))
        .route("/api/fs/read/{*path}", get(fs::read))
        .route("/api/fs/list", get(fs::list))
        .route("/api/fs/find", get(fs::find))
        .route("/api/experimental/fs/write", post(fs::write))
        .route("/api/pty", get(pty::list).post(pty::create))
        .route("/api/pty/{ptyID}", get(pty::get).put(pty::update).delete(pty::remove))
        .route("/api/pty/{ptyID}/connect-token", post(pty::connect_token))
        .route("/api/shell", get(shell::list).post(shell::create))
        .route("/api/shell/{id}", get(shell::get).delete(shell::remove))
        .route("/api/shell/{id}/output", get(shell::output))
        .route("/api/event", get(event::subscribe))
        .route("/api/experimental/usage/limits", get(misc::usage_limits))
        .route("/api/experimental/generate/text", post(misc::generate_text))
        .route("/api/experimental/generate", post(misc::generate_text))
        .route("/api/form", get(permission::empty_forms));

    // Routes reachable without the service password (the pairing flow is
    // completed in a browser, so it cannot present an Authorization header).
    let public = Router::new().route("/auth/connect/{code}", get(misc::connect));

    protected
        .merge(public)
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state)
}

async fn authorize(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path.starts_with("/auth/connect/") {
        return next.run(request).await;
    }
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let authorized = header
        .and_then(auth::basic_password)
        .or_else(|| {
            header
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(str::to_string)
        })
        .map(|password| state.verify(&password))
        .unwrap_or(false);
    if authorized {
        next.run(request).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

/// Converts `anyhow` errors into API responses for handlers that use `?`.
pub fn api<T>(result: anyhow::Result<T>) -> Result<T, ApiError> {
    result.map_err(ApiError::from)
}

/// Small helper for returning `204 No Content`.
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// Helper for returning a raw JSON body without the data envelope.
pub fn json_body(value: Value) -> Json<Value> {
    Json(value)
}

/// Parses a boolean query parameter.
pub fn bool_param(params: &HashMap<String, String>, key: &str) -> bool {
    params
        .get(key)
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false)
}

/// Reads a JSON body into a `serde_json::Value`, rejecting invalid JSON.
pub async fn read_json(body: Body) -> Result<Value, ApiError> {
    let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    if bytes.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_slice(&bytes).map_err(|error| ApiError::invalid(error.to_string()))
}
