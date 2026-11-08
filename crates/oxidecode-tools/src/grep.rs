use super::context::{ToolContext, ToolError, ToolOutput};
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct Input {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include: Option<String>,
    #[serde(default)]
    pub literal: Option<bool>,
}

const MAX_MATCHES: usize = 100;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    if input.pattern.is_empty() {
        return Err(ToolError::invalid("pattern is required"));
    }
    let base = match input.path.as_deref() {
        Some(path) if !path.trim().is_empty() => context.resolve(path)?,
        _ => context.directory.clone(),
    };
    let literal = input.literal.unwrap_or(false);
    let expression = if literal {
        regex::escape(&input.pattern)
    } else {
        input.pattern.clone()
    };
    let regex = Regex::new(&expression)
        .map_err(|error| ToolError::invalid(format!("invalid regular expression: {error}")))?;
    let include: Option<GlobMatcher> = match input.include.as_deref() {
        Some(pattern) if !pattern.trim().is_empty() => Some(
            Glob::new(pattern)
                .map_err(|error| ToolError::invalid(format!("invalid include glob: {error}")))?
                .compile_matcher(),
        ),
        _ => None,
    };

    let base_clone = base.clone();
    let matches = tokio::task::spawn_blocking(move || {
        let mut out: Vec<String> = Vec::new();
        let mut count = 0usize;

        let mut visit_file = |path: &std::path::Path, display: &str| -> bool {
            if let Ok(metadata) = path.metadata() {
                if metadata.len() > MAX_FILE_BYTES {
                    return true;
                }
            }
            let Ok(bytes) = std::fs::read(path) else {
                return true;
            };
            if is_binary(&bytes) {
                return true;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if regex.is_match(line) {
                    let clipped: String = if line.chars().count() > 400 {
                        format!("{}…", line.chars().take(400).collect::<String>())
                    } else {
                        line.to_string()
                    };
                    out.push(format!("{}:{}:{}", display, index + 1, clipped));
                    count += 1;
                    if count >= MAX_MATCHES {
                        return false;
                    }
                }
            }
            true
        };

        if base_clone.is_file() {
            let display = base_clone.display().to_string();
            visit_file(&base_clone, &display);
        } else {
            let walker = ignore::WalkBuilder::new(&base_clone)
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .follow_links(false)
                .build();
            'walk: for entry in walker.flatten() {
                if !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
                    continue;
                }
                let path = entry.path();
                let relative = path.strip_prefix(&base_clone).unwrap_or(path);
                let relative_text = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                if let Some(include) = &include {
                    if !include.is_match(&relative_text) {
                        continue;
                    }
                }
                if !visit_file(path, &relative_text) {
                    break 'walk;
                }
            }
        }
        (out, count)
    })
    .await
    .map_err(|error| ToolError::failed(error.to_string()))?;

    let (lines, count) = matches;
    if count == 0 {
        return Ok(ToolOutput::new(
            format!("No matches for {}", input.pattern),
            format!("No matches found for `{}`.", input.pattern),
            json!({ "pattern": input.pattern, "count": 0 }),
        ));
    }
    let truncated = count >= MAX_MATCHES;
    let mut output = lines.join("\n");
    output.push('\n');
    if truncated {
        output.push_str(&format!("\n[Limited to {MAX_MATCHES} matches.]\n"));
    }
    Ok(ToolOutput::new(
        format!("{} match(es) for {}", count, input.pattern),
        output,
        json!({ "pattern": input.pattern, "count": count, "truncated": truncated }),
    ))
}

fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    sample.contains(&0u8)
}
