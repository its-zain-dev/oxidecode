//! Filesystem endpoints used by the editor, file browser and TUI.

use crate::error::{ApiError, ApiResult};
use crate::routes::{bool_param, located, query_directory, string_param};
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

fn resolve(location: &FsPath, requested: Option<&str>) -> PathBuf {
    let Some(requested) = requested.filter(|value| !value.trim().is_empty()) else {
        return location.to_path_buf();
    };
    let path = PathBuf::from(requested);
    if path.is_absolute() {
        path
    } else {
        location.join(path)
    }
}

fn relative_entry(location: &FsPath, path: &FsPath, is_dir: bool) -> Value {
    let relative = path
        .strip_prefix(location)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    json!({
        "path": relative,
        "type": if is_dir { "directory" } else { "file" },
    })
}

fn mime_for(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "jsonc" => "application/json",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/plain",
        "rs" => "text/plain",
        "c" | "h" => "text/plain",
        "toml" | "yaml" | "yml" | "txt" | "log" | "sh" | "bash" | "zsh" => "text/plain",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

pub async fn read(
    State(state): State<AppState>,
    Path(wildcard): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let directory = query_directory(&state, &params);
    let requested = string_param(&params, "path")
        .map(str::to_string)
        .or_else(|| {
            if wildcard.is_empty() {
                None
            } else if wildcard.starts_with('/') {
                Some(wildcard.clone())
            } else {
                Some(format!("/{wildcard}"))
            }
        });
    let path = resolve(&directory, requested.as_deref());
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| ApiError::not_found(format!("{}: {error}", path.display())))?;
    if metadata.is_dir() {
        return Err(ApiError::invalid(format!(
            "{} is a directory",
            path.display()
        )));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let total = bytes.len() as u64;

    if let Some(range) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes="))
    {
        let mut parts = range.splitn(2, '-');
        let start = parts
            .next()
            .and_then(|value| value.trim().parse::<u64>().ok());
        let end = parts
            .next()
            .and_then(|value| value.trim().parse::<u64>().ok());
        let (start, end) = match (start, end) {
            (Some(start), Some(end)) => (start, end.min(total.saturating_sub(1))),
            (Some(start), None) => (start, total.saturating_sub(1)),
            (None, Some(length)) => (total.saturating_sub(length), total.saturating_sub(1)),
            (None, None) => (0, total.saturating_sub(1)),
        };
        if start >= total || start > end {
            return Ok(StatusCode::RANGE_NOT_SATISFIABLE.into_response());
        }
        let slice = bytes[start as usize..=(end as usize)].to_vec();
        let mut response = Response::new(Body::from(slice));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(mime_for(&path)),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
            headers.insert(header::CONTENT_RANGE, value);
        }
        return Ok(response);
    }

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(mime_for(&path)),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    Ok(response)
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let path = resolve(&directory, string_param(&params, "path"));
    let mut reader = tokio::fs::read_dir(&path)
        .await
        .map_err(|error| ApiError::invalid(format!("{}: {error}", path.display())))?;
    let mut entries: Vec<(bool, String, PathBuf)> = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        entries.push((file_type.is_dir(), name, entry.path()));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let data: Vec<Value> = entries
        .iter()
        .map(|(is_dir, _, path)| relative_entry(&directory, path, *is_dir))
        .collect();
    Ok(located(&directory.display().to_string(), data))
}

pub async fn find(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let query = string_param(&params, "query")
        .ok_or_else(|| ApiError::invalid("query is required"))?
        .to_ascii_lowercase();
    let only = string_param(&params, "type").map(str::to_string);
    let limit = string_param(&params, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let fuzzy = bool_param(&params, "fuzzy");
    let base = directory.clone();
    let query_clone = query.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let mut found: Vec<(i64, usize, bool, PathBuf)> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(false)
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            if path == base {
                continue;
            }
            let is_dir = entry
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false);
            let relative = path
                .strip_prefix(&base)
                .unwrap_or(path)
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let lower = relative.to_ascii_lowercase();
            let score = if lower.starts_with(&query_clone) {
                0
            } else if lower.contains(&query_clone) {
                1
            } else if fuzzy && fuzzy_match(&query_clone, &lower) {
                3
            } else {
                continue;
            };
            found.push((score, relative.len(), is_dir, path.to_path_buf()));
            if found.len() > 20_000 {
                break;
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        found
            .into_iter()
            .filter(|(_, _, is_dir, _)| match only.as_deref() {
                Some("file") => !*is_dir,
                Some("directory") => *is_dir,
                _ => true,
            })
            .take(limit)
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let data: Vec<Value> = entries
        .into_iter()
        .map(|(_, _, is_dir, path)| relative_entry(&directory, &path, is_dir))
        .collect();
    Ok(located(&directory.display().to_string(), data))
}

fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    'outer: for wanted in needle.chars() {
        for candidate in chars.by_ref() {
            if candidate == wanted {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

pub async fn write(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let directory = query_directory(&state, &params);
    let requested = string_param(&params, "path")
        .ok_or_else(|| ApiError::invalid("path is required"))?;
    let path = resolve(&directory, Some(requested));
    let absolute = path.display().to_string();
    tokio::task::spawn_blocking({
        let path = path.clone();
        let body = body.to_vec();
        move || oxidecode_core::util::atomic_write(&path, &body)
    })
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?
    .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(located(
        &directory.display().to_string(),
        json!({ "path": absolute }),
    ))
}
