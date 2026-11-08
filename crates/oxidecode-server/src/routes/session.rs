//! Session endpoints: CRUD, prompts, messages, model/agent switching and the
//! session-scoped inbox.

use crate::error::{ApiError, ApiResult};
use crate::routes::{data, no_content, query_directory, string_param};
use crate::runner;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use oxidecode_core::ids;
use oxidecode_core::session::{
    AssistantPart, LocationRef, Message, MessageTime, ModelRef, SessionInfo, ShellMessage,
    SyntheticMessage, UserMessage,
};
use oxidecode_core::store::ProjectInfo;
use oxidecode_core::vcs;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

fn parse_model(value: &Value) -> Option<ModelRef> {
    if let Some(text) = value.as_str() {
        let (provider, rest) = text.split_once('/')?;
        let (id, variant) = match rest.split_once('#') {
            Some((id, variant)) if !variant.is_empty() => {
                (id.to_string(), Some(variant.to_string()))
            }
            _ => (rest.to_string(), None),
        };
        if provider.is_empty() || id.is_empty() {
            return None;
        }
        return Some(ModelRef {
            id,
            provider_id: provider.to_string(),
            variant,
        });
    }
    let object = value.as_object()?;
    let provider_id = object
        .get("providerID")
        .or_else(|| object.get("providerId"))
        .and_then(Value::as_str)?
        .to_string();
    let id = object
        .get("id")
        .or_else(|| object.get("modelID"))
        .and_then(Value::as_str)?
        .to_string();
    let variant = object
        .get("variant")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(ModelRef {
        id,
        provider_id,
        variant,
    })
}

fn parse_location(state: &AppState, value: Option<&Value>) -> PathBuf {
    let requested = value.and_then(|value| {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("directory")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    });
    let directory = state.location(requested.as_deref());
    let _ = std::fs::create_dir_all(&directory);
    directory
        .canonicalize()
        .unwrap_or_else(|_| directory.clone())
}

fn session_or_404(state: &AppState, id: &str) -> ApiResult<SessionInfo> {
    state
        .store()
        .get_session(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::session_not_found(id))
}

fn project_for(state: &AppState, directory: &std::path::Path) -> ApiResult<ProjectInfo> {
    state
        .store()
        .ensure_project(directory)
        .map_err(ApiError::from)
}

fn derive_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("New session");
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title.push('…');
    }
    title
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let directory = parse_location(&state, body.get("location"));
    let project = project_for(&state, &directory)?;
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("New session")
        .to_string();
    let agent = body
        .get("agent")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let model = body.get("model").and_then(parse_model);
    let mut session = SessionInfo::new(
        project.id,
        directory.display().to_string(),
        title,
        agent,
        model,
    );
    if let Some(id) = body.get("id").and_then(Value::as_str) {
        if !id.is_empty() {
            session.id = id.to_string();
        }
    }
    if let Some(parent) = body.get("parentID").and_then(Value::as_str) {
        if !parent.is_empty() {
            session.parent_id = Some(parent.to_string());
        }
    }
    session.metadata = body.get("metadata").cloned().filter(|value| !value.is_null());
    session.permissions = body
        .get("permissions")
        .cloned()
        .filter(|value| !value.is_null());
    state.store().create_session(&session).map_err(ApiError::from)?;
    Ok(data(session))
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let limit = string_param(&params, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let order = string_param(&params, "order").unwrap_or("desc");
    let search = string_param(&params, "search");
    let directory = string_param(&params, "directory")
        .map(|value| state.location(Some(value)))
        .map(|path| path.display().to_string());
    let archived = params
        .get("archived")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    let sessions = state
        .store()
        .list_sessions(None, directory.as_deref(), search, limit, archived, order)
        .map_err(ApiError::from)?;
    let parent = string_param(&params, "parentID");
    let filtered: Vec<SessionInfo> = match parent {
        Some("null") => sessions
            .into_iter()
            .filter(|session| session.parent_id.is_none())
            .collect(),
        Some(id) => sessions
            .into_iter()
            .filter(|session| session.parent_id.as_deref() == Some(id))
            .collect(),
        None => sessions,
    };
    Ok(Json(json!({
        "data": filtered,
        "cursor": { "previous": Value::Null, "next": Value::Null },
    })))
}

pub async fn get(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &session_id)?;
    Ok(data(session))
}

pub async fn update(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mut session = session_or_404(&state, &session_id)?;
    let mut renamed = false;
    if let Some(title) = body.get("title").and_then(Value::as_str) {
        if session.title != title {
            session.title = title.to_string();
            renamed = true;
        }
    }
    if body.get("metadata").is_some() {
        session.metadata = body
            .get("metadata")
            .cloned()
            .filter(|value| !value.is_null());
    }
    if body.get("permissions").is_some() {
        session.permissions = body
            .get("permissions")
            .cloned()
            .filter(|value| !value.is_null());
    }
    if let Some(archived) = body.get("archived").and_then(Value::as_bool) {
        session.time.updated = ids::now_ms();
        if archived {
            session.time.archived = Some(ids::now_ms());
        } else {
            session.time.archived = None;
        }
    }
    state.store().update_session(&session).map_err(ApiError::from)?;
    if renamed {
        state.bus().emit_session(
            "session.renamed",
            &session.location.directory,
            json!({ "sessionID": session.id, "title": session.title }),
        );
    }
    if body.get("archived").is_some() {
        let kind = if session.time.archived.is_some() {
            "session.archived"
        } else {
            "session.unarchived"
        };
        state.bus().emit_session(
            kind,
            &session.location.directory,
            json!({ "sessionID": session.id }),
        );
    }
    Ok(no_content())
}

pub async fn remove(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Response> {
    let _ = session_or_404(&state, &session_id)?;
    state
        .store()
        .delete_session(&session_id)
        .map_err(ApiError::from)?;
    Ok(no_content())
}

pub async fn active(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut data = serde_json::Map::new();
    for (session_id, _message, _started) in state.running_sessions() {
        data.insert(session_id, json!({ "type": "running" }));
    }
    Ok(Json(json!({ "data": data })))
}

pub async fn fork(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let parent = session_or_404(&state, &session_id)?;
    let mut child = SessionInfo::new(
        parent.project_id.clone(),
        parent.location.directory.clone(),
        parent.title.clone(),
        parent.agent.clone(),
        parent.model.clone(),
    );
    child.parent_id = Some(parent.id.clone());
    child.metadata = parent.metadata.clone();
    state.store().create_session(&child).map_err(ApiError::from)?;

    let before = body.get("before").and_then(Value::as_str);
    let messages = state
        .store()
        .all_messages(&session_id)
        .map_err(ApiError::from)?;
    for message in messages {
        if let Some(before) = before {
            if message.id() == before {
                break;
            }
        }
        let mut value = serde_json::to_value(&message).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("id".into(), json!(ids::message()));
        }
        if let Ok(cloned) = serde_json::from_value::<Message>(value) {
            let _ = state.store().append_message(&child.id, &cloned);
        }
    }
    state.bus().emit_session(
        "session.forked",
        &child.location.directory,
        json!({ "sessionID": child.id, "parentID": parent.id }),
    );
    Ok(data(child))
}

pub async fn switch_agent(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mut session = session_or_404(&state, &session_id)?;
    let agent = body
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::invalid("agent is required"))?;
    session.agent = Some(agent.clone());
    state.store().update_session(&session).map_err(ApiError::from)?;
    state.bus().emit_session(
        "session.agent.selected",
        &session.location.directory,
        json!({ "sessionID": session.id, "agent": agent }),
    );
    Ok(no_content())
}

pub async fn switch_model(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mut session = session_or_404(&state, &session_id)?;
    let model = body
        .get("model")
        .and_then(parse_model)
        .ok_or_else(|| ApiError::invalid("model is required"))?;
    session.model = Some(model.clone());
    state.store().update_session(&session).map_err(ApiError::from)?;
    state.bus().emit_session(
        "session.model.selected",
        &session.location.directory,
        json!({ "sessionID": session.id, "model": model }),
    );
    Ok(no_content())
}

pub async fn move_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mut session = session_or_404(&state, &session_id)?;
    let directory = body
        .get("directory")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("directory is required"))?;
    let directory = state
        .location(Some(directory))
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(directory));
    let project = project_for(&state, &directory)?;
    session.location = LocationRef::new(directory.display().to_string());
    session.project_id = project.id;
    state.store().update_session(&session).map_err(ApiError::from)?;
    state.bus().emit_session(
        "session.moved",
        &session.location.directory,
        json!({ "sessionID": session.id, "directory": session.location.directory }),
    );
    Ok(no_content())
}

/// Shared prompt delivery used by `/prompt` and `/command`.
fn deliver_prompt(
    state: &AppState,
    session: &mut SessionInfo,
    text: String,
    files: Value,
    agents: Value,
    skills: Value,
    metadata: Option<Value>,
    message_id: Option<String>,
    delivery: Option<String>,
) -> ApiResult<Json<Value>> {
    let text = text.trim_end().to_string();
    let files = files.as_array().cloned().unwrap_or_default();
    let agents = agents.as_array().cloned().unwrap_or_default();
    let skills = skills.as_array().cloned().unwrap_or_default();
    if text.trim().is_empty() && files.is_empty() {
        return Err(ApiError::invalid("text or files are required"));
    }

    let id = message_id
        .filter(|value| !value.is_empty())
        .unwrap_or_else(ids::message);
    let now = ids::now_ms();
    let user = UserMessage {
        id: id.clone(),
        metadata: metadata.clone(),
        time: MessageTime {
            created: now,
            streamed: None,
            completed: None,
        },
        text: text.clone(),
        files: files.clone(),
        agents: agents.clone(),
        skills: skills.clone(),
    };
    state
        .store()
        .append_message(&session.id, &Message::User(user))
        .map_err(ApiError::from)?;

    // Name the session after its first prompt.
    if session.title.is_empty() || session.title == "New session" {
        session.title = derive_title(&text);
        session.time.updated = now;
        let _ = state.store().update_session(session);
        state.bus().emit_session(
            "session.renamed",
            &session.location.directory,
            json!({ "sessionID": session.id, "title": session.title }),
        );
    }

    let busy = state.is_running(&session.id);
    let delivery = delivery
        .filter(|value| value == "steer" || value == "queue")
        .unwrap_or_else(|| "queue".to_string());
    if busy {
        state.queue_prompt(&session.id, &id);
    }

    let payload = json!({
        "text": text,
        "files": files,
        "agents": agents,
        "skills": skills,
        "metadata": metadata,
    });
    let inbox = json!({
        "id": id,
        "sessionID": session.id,
        "time": { "created": now },
        "type": "user",
        "payload": payload,
        "delivery": delivery,
    });
    state.bus().emit_session(
        if busy {
            "session.inbox.enqueued"
        } else {
            "session.inbox.delivered"
        },
        &session.location.directory,
        json!({ "sessionID": session.id, "inboxID": id, "item": inbox }),
    );

    if !busy {
        runner::spawn(state.clone(), session.id.clone());
    }
    Ok(data(inbox))
}

pub async fn prompt(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let mut session = session_or_404(&state, &session_id)?;
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    deliver_prompt(
        &state,
        &mut session,
        text,
        body.get("files").cloned().unwrap_or(json!([])),
        body.get("agents").cloned().unwrap_or(json!([])),
        body.get("skills").cloned().unwrap_or(json!([])),
        body.get("metadata").cloned().filter(|value| !value.is_null()),
        body.get("id").and_then(Value::as_str).map(str::to_string),
        body.get("delivery").and_then(Value::as_str).map(str::to_string),
    )
}

pub async fn command(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let mut session = session_or_404(&state, &session_id)?;
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("name is required"))?;
    let arguments = body.get("text").and_then(Value::as_str).unwrap_or("");
    let config = state.config_for(&PathBuf::from(&session.location.directory));
    let template = config
        .config
        .commands
        .get(name)
        .and_then(|command| command.template.clone())
        .unwrap_or_else(|| format!("/{name} {arguments}"));
    let text = template
        .replace("$ARGUMENTS", arguments)
        .replace("$ARGS", arguments)
        .replace("{arguments}", arguments);
    deliver_prompt(
        &state,
        &mut session,
        text,
        body.get("files").cloned().unwrap_or(json!([])),
        body.get("agents").cloned().unwrap_or(json!([])),
        body.get("skills").cloned().unwrap_or(json!([])),
        None,
        body.get("id").and_then(Value::as_str).map(str::to_string),
        body.get("delivery").and_then(Value::as_str).map(str::to_string),
    )?;
    Ok(no_content())
}

pub async fn synthetic(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &session_id)?;
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let description = body
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = ids::message();
    let now = ids::now_ms();
    let message = SyntheticMessage {
        id: id.clone(),
        metadata: None,
        time: MessageTime {
            created: now,
            streamed: None,
            completed: None,
        },
        text: text.clone(),
        description: description.clone(),
    };
    state
        .store()
        .append_message(&session.id, &Message::Synthetic(message))
        .map_err(ApiError::from)?;
    let inbox = json!({
        "id": id,
        "sessionID": session.id,
        "time": { "created": now },
        "type": "synthetic",
        "payload": { "text": text, "description": description },
        "delivery": "queue",
    });
    state.bus().emit_session(
        "session.synthetic",
        &session.location.directory,
        json!({ "sessionID": session.id, "inboxID": id, "item": inbox }),
    );
    Ok(data(inbox))
}

pub async fn shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let session = session_or_404(&state, &session_id)?;
    let command = body
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if command.trim().is_empty() {
        return Err(ApiError::invalid("command is required"));
    }
    let config = state.config_for(&PathBuf::from(&session.location.directory));
    let shell = config
        .config
        .shell
        .clone()
        .unwrap_or_else(|| "/system/bin/sh".to_string());
    let shell_id = ids::prefixed("shl");
    let started = ids::now_ms();
    state.bus().emit_session(
        "session.shell.started",
        &session.location.directory,
        json!({ "sessionID": session.id, "shellID": shell_id, "command": command }),
    );
    let output = tokio::process::Command::new(&shell)
        .arg("-c")
        .arg(&command)
        .current_dir(&session.location.directory)
        .env("NO_COLOR", "1")
        .output()
        .await;
    let (status, exit, text) = match output {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            (
                if output.status.success() {
                    "completed".to_string()
                } else {
                    "failed".to_string()
                },
                output.status.code(),
                text,
            )
        }
        Err(error) => ("failed".to_string(), None, error.to_string()),
    };
    let message = ShellMessage {
        id: ids::message(),
        metadata: None,
        time: MessageTime {
            created: started,
            streamed: None,
            completed: Some(ids::now_ms()),
        },
        shell_id: shell_id.clone(),
        command: command.clone(),
        status: status.clone(),
        exit: exit.map(|code| json!(code)),
        output: Some(json!({ "text": text })),
    };
    let _ = state
        .store()
        .append_message(&session.id, &Message::Shell(message));
    state.bus().emit_session(
        "session.shell.ended",
        &session.location.directory,
        json!({
            "sessionID": session.id,
            "shellID": shell_id,
            "status": status,
            "exit": exit,
            "output": text,
        }),
    );
    Ok(no_content())
}

pub async fn compact(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(_body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &session_id)?;
    let id = ids::message();
    let now = ids::now_ms();
    let inbox = json!({
        "id": id,
        "sessionID": session.id,
        "time": { "created": now },
        "type": "compaction",
        "payload": {},
        "delivery": "queue",
    });
    state.bus().emit_session(
        "session.compaction.started",
        &session.location.directory,
        json!({ "sessionID": session.id, "messageID": id }),
    );
    state.bus().emit_session(
        "session.compaction.ended",
        &session.location.directory,
        json!({ "sessionID": session.id, "messageID": id }),
    );
    Ok(data(inbox))
}

pub async fn generate(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _session = session_or_404(&state, &session_id)?;
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = derive_title(prompt);
    Ok(Json(json!({ "data": { "text": text } })))
}

pub async fn interrupt(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let interrupted = state.interrupt(&session_id);
    Ok(Json(json!({ "interrupted": interrupted })))
}

pub async fn messages(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &session_id)?;
    let limit = string_param(&params, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(500)
        .clamp(1, 10_000);
    let order = string_param(&params, "order").unwrap_or("asc");
    let messages = state
        .store()
        .list_messages(&session_id, limit, order, None)
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "data": messages,
        "cursor": { "previous": Value::Null, "next": Value::Null },
    })))
}

pub async fn message(
    State(state): State<AppState>,
    Path((session_id, message_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let messages = state
        .store()
        .all_messages(&session_id)
        .map_err(ApiError::from)?;
    let found = messages
        .into_iter()
        .find(|message| message.id() == message_id)
        .ok_or_else(|| {
            ApiError::not_found(format!("message {message_id} not found in session {session_id}"))
        })?;
    Ok(data(found))
}

pub async fn context(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &session_id)?;
    let messages = state
        .store()
        .all_messages(&session_id)
        .map_err(ApiError::from)?;
    Ok(data(messages))
}

pub async fn diff(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &session_id)?;
    let directory = PathBuf::from(&session.location.directory);
    let diffs = tokio::task::spawn_blocking(move || vcs::diff(&directory, "working"))
        .await
        .unwrap_or_default();
    Ok(data(diffs))
}

pub async fn inbox(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &session_id)?;
    let pending = state.pending_count(&session_id);
    Ok(Json(json!({
        "data": [],
        "pending": pending,
    })))
}

pub async fn update_inbox(
    State(state): State<AppState>,
    Path((session_id, _inbox_id)): Path<(String, String)>,
    Json(_body): Json<Value>,
) -> ApiResult<Response> {
    let _ = session_or_404(&state, &session_id)?;
    Ok(no_content())
}

pub async fn cancel_inbox(
    State(state): State<AppState>,
    Path((session_id, _inbox_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    let _ = session_or_404(&state, &session_id)?;
    Ok(no_content())
}

pub async fn revert_stage(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &session_id)?;
    let message_id = body
        .get("messageID")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("messageID is required"))?;
    Ok(Json(json!({
        "data": {
            "messageID": message_id,
            "files": [],
        }
    })))
}

pub async fn revert_commit(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Response> {
    let session = session_or_404(&state, &session_id)?;
    state.bus().emit_session(
        "session.revert.committed",
        &session.location.directory,
        json!({ "sessionID": session.id }),
    );
    Ok(no_content())
}

pub async fn revert_clear(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Response> {
    let session = session_or_404(&state, &session_id)?;
    state.bus().emit_session(
        "session.revert.cleared",
        &session.location.directory,
        json!({ "sessionID": session.id }),
    );
    Ok(no_content())
}

pub async fn view(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let _ = query_directory(&state, &params);
    let mut session = session_or_404(&state, &session_id)?;
    session.time.viewed = Some(ids::now_ms());
    state.store().update_session(&session).map_err(ApiError::from)?;
    state.bus().emit_session(
        "session.viewed",
        &session.location.directory,
        json!({ "sessionID": session.id }),
    );
    Ok(no_content())
}

pub async fn export(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &session_id)?;
    let messages = state
        .store()
        .all_messages(&session_id)
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "data": { "info": session, "messages": messages }
    })))
}

pub async fn import(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let info_value = body
        .get("info")
        .cloned()
        .ok_or_else(|| ApiError::invalid("info is required"))?;
    let mut session: SessionInfo = serde_json::from_value(info_value)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    if state
        .store()
        .session_exists(&session.id)
        .unwrap_or(false)
    {
        session.id = ids::session();
    }
    state.store().create_session(&session).map_err(ApiError::from)?;
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for value in messages {
            if let Ok(message) = serde_json::from_value::<Message>(value.clone()) {
                let _ = state.store().append_message(&session.id, &message);
            }
        }
    }
    Ok(data(session))
}

pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let sessions = state
        .store()
        .list_sessions(None, None, None, 10_000, true, "desc")
        .map_err(ApiError::from)?;
    let mut prompts = 0usize;
    let mut steps = 0usize;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut reasoning = 0u64;
    let mut cost = 0f64;
    for session in &sessions {
        if let Ok(messages) = state.store().all_messages(&session.id) {
            for message in &messages {
                match message {
                    Message::User(_) => prompts += 1,
                    Message::Assistant(assistant) => {
                        steps += 1;
                        if let Some(tokens) = &assistant.tokens {
                            input += tokens.input;
                            output += tokens.output;
                            reasoning += tokens.reasoning;
                        }
                        cost += assistant.cost.unwrap_or(0.0);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(Json(json!({
        "data": {
            "range": { "from": 0, "to": ids::now_ms() },
            "sessions": sessions.len(),
            "subagents": sessions.iter().filter(|session| session.parent_id.is_some()).count(),
            "prompts": prompts,
            "steps": steps,
            "tokens": { "input": input, "output": output, "reasoning": reasoning, "cache": { "read": 0, "write": 0 } },
            "cost": cost,
            "tools": {},
            "activeDays": 0,
            "streak": 0,
            "activity": [],
            "models": [],
        }
    })))
}

/// Ensures assistant parts are never empty when a client asks for the raw
/// content shape (used by the TUI when restoring old sessions).
pub fn content_json(content: &[AssistantPart]) -> Value {
    serde_json::to_value(content).unwrap_or_else(|_| json!([]))
}
