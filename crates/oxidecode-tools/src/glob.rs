use super::context::{ToolContext, ToolError, ToolOutput};
use globset::{Glob, GlobMatcher};
use serde::Deserialize;
use serde_json::json;
use std::time::SystemTime;

#[derive(Debug, Deserialize)]
pub struct Input {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
}

const MAX_RESULTS: usize = 100;

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let base = match input.path.as_deref() {
        Some(path) if !path.trim().is_empty() => context.resolve(path)?,
        _ => context.directory.clone(),
    };
    let mut pattern = input.pattern.trim().to_string();
    if pattern.is_empty() {
        return Err(ToolError::invalid("pattern is required"));
    }
    if pattern.starts_with('/') {
        pattern = pattern.trim_start_matches('/').to_string();
    }
    let matcher: GlobMatcher = Glob::new(&pattern)
        .map_err(|error| ToolError::invalid(format!("invalid pattern: {error}")))?
        .compile_matcher();

    let base_clone = base.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let mut matches: Vec<(String, Option<SystemTime>)> = Vec::new();
        let walker = ignore::WalkBuilder::new(&base_clone)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(false)
            .build();
        for entry in walker.flatten() {
            if matches.len() >= MAX_RESULTS * 4 {
                break;
            }
            let path = entry.path();
            if path == base_clone || !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
                continue;
            }
            let relative = path.strip_prefix(&base_clone).unwrap_or(path);
            let relative_text = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if matcher.is_match(&relative_text) {
                let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
                matches.push((relative_text, modified));
            }
        }
        matches
    })
    .await
    .map_err(|error| ToolError::failed(error.to_string()))?;

    let mut entries = entries;
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(MAX_RESULTS);

    let base_display = context.relative(&base);
    if entries.is_empty() {
        return Ok(ToolOutput::new(
            format!("No files matched {pattern}"),
            format!("No files matched {pattern} in {base_display}."),
            json!({ "pattern": pattern, "count": 0, "truncated": false }),
        ));
    }

    let truncated = entries.len() >= MAX_RESULTS;
    let listing = entries
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = format!("Found {} file(s) in {base_display}:\n{listing}\n", entries.len());
    if truncated {
        output.push_str(&format!("\n[Limited to {MAX_RESULTS} results.]\n"));
    }
    let title = format!("{} file(s) matched {pattern}", entries.len());
    Ok(ToolOutput::new(
        title,
        output,
        json!({ "pattern": pattern, "count": entries.len(), "truncated": truncated }),
    ))
}
