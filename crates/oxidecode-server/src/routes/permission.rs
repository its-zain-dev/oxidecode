//! Permission request endpoints.

use crate::error::{ApiError, ApiResult};
use crate::routes::{located, no_content, query_directory};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use oxidecode_core::ids;
use oxidecode_core::store::PermissionInfo;
use serde_json::{json, Value};
use std::collections::HashMap;

pub async fn list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let permissions = state
        .store()
        .list_permissions(&session_id)
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "data": permissions })))
}

pub async fn get(
    State(state): State<AppState>,
    Path((session_id, request_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let permission = state
        .store()
        .get_permission(&session_id, &request_id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::permission_not_found(&request_id))?;
    Ok(Json(json!({ "data": permission })))
}

pub async fn reply(
    State(state): State<AppState>,
    Path((session_id, request_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let decision = body
        .get("decision")
        .and_then(Value::as_str)
        .or_else(|| body.get("reply").and_then(Value::as_str))
        .unwrap_or("reject");
    let mapped = match decision {
        "once" | "always" | "reject" => decision,
        "allow" | "yes" | "true" => "once",
        "deny" | "no" | "false" => "reject",
        _ => return Err(ApiError::invalid("decision must be once, always or reject")),
    };
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);
    crate::permission::reply(&state, &session_id, &request_id, mapped, message)?;
    Ok(no_content())
}

pub async fn create(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("bash")
        .to_string();
    let resources: Vec<String> = body
        .get("resources")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let id = ids::permission();
    let permission = PermissionInfo {
        id: id.clone(),
        session_id: session_id.clone(),
        action,
        resources,
        save: Vec::new(),
        metadata: body.get("metadata").cloned(),
        source: Some(json!({ "type": "api" })),
        message: None,
        decision: None,
    };
    state
        .store()
        .create_permission(&permission)
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "data": { "id": id, "effect": "ask" } })))
}

pub async fn list_all(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let sessions = state
        .store()
        .list_sessions(None, None, None, 5_000, true, "desc")
        .map_err(ApiError::from)?;
    let mut permissions = Vec::new();
    for session in sessions {
        if let Ok(mut pending) = state.store().list_permissions(&session.id) {
            permissions.append(&mut pending);
        }
    }
    Ok(located(&directory.display().to_string(), permissions))
}

pub async fn saved(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let _ = state;
    Ok(Json(json!({ "data": [] })))
}

pub async fn update_saved(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _ = state;
    let now = ids::now_ms();
    Ok(Json(json!({
        "data": {
            "id": id,
            "projectID": body.get("projectID").and_then(Value::as_str).unwrap_or(""),
            "action": body.get("action").and_then(Value::as_str).unwrap_or("*"),
            "resource": body.get("resource").and_then(Value::as_str).unwrap_or("*"),
            "time": { "created": now, "updated": now },
        }
    })))
}

pub async fn remove_saved(
    State(state): State<AppState>,
    Path(_id): Path<String>,
) -> ApiResult<Response> {
    let _ = state;
    Ok(no_content())
}

pub async fn empty_forms(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let _ = state;
    Ok(Json(json!({ "data": [] })))
}
