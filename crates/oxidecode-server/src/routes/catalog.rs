//! Model, provider, agent and command catalog endpoints.

use crate::error::ApiResult;
use crate::routes::{located, query_directory};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use oxidecode_provider::catalog;
use serde_json::{json, Value};
use std::collections::HashMap;

fn location_params(state: &AppState, params: &HashMap<String, String>) -> String {
    query_directory(state, params).display().to_string()
}

pub async fn models(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let models = catalog::models(&config);
    Ok(located(&directory.display().to_string(), models))
}

pub async fn default_model(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let model = catalog::default_model(&config);
    Ok(located(&directory.display().to_string(), model))
}

pub async fn providers(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let providers: Vec<Value> = catalog::providers(&config)
        .into_iter()
        .map(|provider| {
            let mut value = serde_json::to_value(provider).unwrap_or(Value::Null);
            if let Some(object) = value.as_object_mut() {
                object.insert("canonical".into(), object.get("id").cloned().unwrap_or(Value::Null));
                object.insert("compatibility".into(), json!({}));
                object.insert("headers".into(), json!({}));
                object.insert("body".into(), json!({}));
            }
            value
        })
        .collect();
    Ok(located(&directory.display().to_string(), providers))
}

pub async fn provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let found = catalog::providers(&config)
        .into_iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| crate::error::ApiError::not_found(format!("provider {provider_id} not found")));
    let mut value = serde_json::to_value(found?).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("canonical".into(), object.get("id").cloned().unwrap_or(Value::Null));
        object.insert("compatibility".into(), json!({}));
        object.insert("headers".into(), json!({}));
        object.insert("body".into(), json!({}));
    }
    Ok(located(&directory.display().to_string(), value))
}

fn default_agents() -> Vec<Value> {
    let make = |id: &str, name: &str, description: &str, mode: &str, hidden: bool| {
        json!({
            "id": id,
            "name": name,
            "request": { "settings": {}, "headers": {}, "body": {} },
            "system": "",
            "description": description,
            "mode": mode,
            "hidden": hidden,
            "permissions": [],
        })
    };
    vec![
        make(
            "build",
            "Build",
            "The default agent. Writes code and runs commands.",
            "primary",
            false,
        ),
        make(
            "plan",
            "Plan",
            "Read-only analysis and planning.",
            "primary",
            false,
        ),
        make(
            "explore",
            "Explore",
            "Searches the codebase and reports findings.",
            "subagent",
            true,
        ),
    ]
}

pub async fn agents(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let mut agents = default_agents();
    for (id, agent) in &config.config.agents {
        if let Some(existing) = agents.iter_mut().find(|value| {
            value.get("id").and_then(Value::as_str) == Some(id.as_str())
        }) {
            if let Some(object) = existing.as_object_mut() {
                if let Some(description) = &agent.description {
                    object.insert("description".into(), json!(description));
                }
                if let Some(mode) = &agent.mode {
                    object.insert("mode".into(), json!(mode));
                }
                if let Some(hidden) = agent.hidden {
                    object.insert("hidden".into(), json!(hidden));
                }
                if let Some(system) = &agent.system {
                    object.insert("system".into(), json!(system));
                }
                if let Some(model) = &agent.model {
                    if let Some(reference) = config.parse_model(model) {
                        object.insert(
                            "model".into(),
                            json!({
                                "id": reference.1,
                                "providerID": reference.0,
                                "variant": reference.2,
                            }),
                        );
                    }
                }
            }
        } else {
            agents.push(json!({
                "id": id,
                "name": agent
                    .description
                    .clone()
                    .unwrap_or_else(|| id.clone()),
                "request": { "settings": {}, "headers": {}, "body": {} },
                "system": agent.system.clone().unwrap_or_default(),
                "description": agent.description.clone().unwrap_or_default(),
                "mode": agent.mode.clone().unwrap_or_else(|| "subagent".to_string()),
                "hidden": agent.hidden.unwrap_or(false),
                "permissions": agent.permissions.clone().unwrap_or_default(),
            }));
        }
    }
    Ok(located(&directory.display().to_string(), agents))
}

pub async fn agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let Json(body) = agents(State(state.clone()), Query(params.clone())).await?;
    let list = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let found = list
        .into_iter()
        .find(|value| value.get("id").and_then(Value::as_str) == Some(agent_id.as_str()))
        .ok_or_else(|| crate::error::ApiError::not_found(format!("agent {agent_id} not found")))?;
    let _ = location_params(&state, &params);
    Ok(located(&directory.display().to_string(), found))
}

pub async fn commands(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let commands: Vec<Value> = config
        .config
        .commands
        .iter()
        .map(|(name, command)| {
            json!({
                "name": name,
                "description": command.description.clone().unwrap_or_default(),
            })
        })
        .collect();
    Ok(located(&directory.display().to_string(), commands))
}

pub async fn skills(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let mut skills = Vec::new();
    let base = state.paths().opencode_config.join("skill");
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let file = path.join("SKILL.md");
            let content = std::fs::read_to_string(&file).unwrap_or_default();
            let description = content
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
                .unwrap_or("")
                .trim()
                .to_string();
            skills.push(json!({
                "id": name,
                "name": name,
                "description": description,
                "autoinvoke": false,
                "path": file.display().to_string(),
                "content": content,
            }));
        }
    }
    Ok(located(&directory.display().to_string(), skills))
}

pub async fn empty_list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    Ok(located(&directory.display().to_string(), Vec::<Value>::new()))
}

pub async fn mcp(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let mut servers = Vec::new();
    if let Some(object) = config.config.mcp.as_object() {
        for (name, value) in object {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("local");
            servers.push(json!({
                "name": name,
                "status": "disabled",
                "type": kind,
            }));
        }
    }
    Ok(located(&directory.display().to_string(), servers))
}

pub async fn integrations(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let config = state.config_for(&directory);
    let data: Vec<Value> = catalog::providers(&config)
        .into_iter()
        .filter(|provider| provider.id != "mock")
        .map(|provider| {
            json!({
                "id": provider.id,
                "name": provider.name,
                "metadata": { "package": provider.package },
                "methods": [],
                "connections": [],
            })
        })
        .collect();
    Ok(located(&directory.display().to_string(), data))
}

pub async fn credentials(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let _ = state;
    Ok(Json(json!({ "data": [] })))
}

pub async fn references(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    Ok(located(&directory.display().to_string(), Vec::<Value>::new()))
}

pub async fn websearch_providers(
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let _ = state;
    Ok(Json(json!({ "data": [] })))
}
