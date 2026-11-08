use super::context::{PermissionRequest, ToolContext, ToolError, ToolOutput};
use super::path::display_relative;
use serde::Deserialize;
use serde_json::json;
use similar::TextDiff;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub file_path: String,
    pub content: String,
}

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let path = context.resolve_for_write(&input.file_path)?;
    if context.aborted() {
        return Err(ToolError::failed("aborted"));
    }
    let relative = display_relative(&context.directory, &path);
    context
        .authorize(PermissionRequest {
            action: "write".into(),
            resources: vec![relative.clone()],
            save: vec![relative.clone()],
            metadata: Some(json!({ "filePath": relative, "bytes": input.content.len() })),
        })
        .await?;

    let previous = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| ToolError::failed(error.to_string()))?;
    }
    let content = input.content;
    tokio::task::spawn_blocking({
        let path = path.clone();
        let content = content.clone();
        move || oxidecode_core::util::atomic_write(&path, content.as_bytes())
    })
    .await
    .map_err(|error| ToolError::failed(error.to_string()))?
    .map_err(|error| ToolError::failed(error.to_string()))?;

    let patch = unified_diff(&relative, &previous, &content);
    let additions = patch.lines().filter(|line| line.starts_with('+') && !line.starts_with("+++")).count();
    let deletions = patch.lines().filter(|line| line.starts_with('-') && !line.starts_with("---")).count();

    Ok(ToolOutput::new(
        format!("Wrote {relative}"),
        format!(
            "Wrote {} ({} bytes).\n\n{patch}",
            relative,
            content.len()
        ),
        json!({
            "filePath": relative,
            "exists": !previous.is_empty() || path.exists(),
            "created": previous.is_empty(),
            "additions": additions,
            "deletions": deletions,
            "patch": patch,
        }),
    ))
}

pub fn unified_diff(file: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    let mut any = false;
    for (index, group) in diff.grouped_ops(3).into_iter().enumerate() {
        any = true;
        if index == 0 {
            out.push_str(&format!("--- a/{file}\n+++ b/{file}\n"));
        }
        for op in group {
            let (_, old_range, new_range) = op.as_tag_tuple();
            let old_start = old_range.start + 1;
            let old_end = old_range.end;
            let new_start = new_range.start + 1;
            let new_end = new_range.end;
            out.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                old_start,
                old_end.saturating_sub(old_range.start),
                new_start,
                new_end.saturating_sub(new_range.start)
            ));
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    similar::ChangeTag::Delete => '-',
                    similar::ChangeTag::Insert => '+',
                    similar::ChangeTag::Equal => ' ',
                };
                out.push(sign);
                out.push_str(change.value());
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    if !any {
        out.push_str(&format!("--- a/{file}\n+++ b/{file}\n(no changes)\n"));
    }
    out
}
