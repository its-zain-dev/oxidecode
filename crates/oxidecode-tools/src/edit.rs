use super::context::{PermissionRequest, ToolContext, ToolError, ToolOutput};
use super::path::display_relative;
use super::write::unified_diff;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let path = context.resolve(&input.file_path)?;
    if context.aborted() {
        return Err(ToolError::failed("aborted"));
    }
    let relative = display_relative(&context.directory, &path);
    context
        .authorize(PermissionRequest {
            action: "edit".into(),
            resources: vec![relative.clone()],
            save: vec![relative.clone()],
            metadata: Some(json!({ "filePath": relative })),
        })
        .await?;

    let original = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| ToolError::invalid(format!("{}: {error}", relative)))?;

    if input.old_string == input.new_string {
        return Err(ToolError::invalid("oldString and newString are identical"));
    }
    if input.old_string.is_empty() {
        return Err(ToolError::invalid(
            "oldString is empty; use the write tool to create files",
        ));
    }

    let occurrences = original.matches(&input.old_string).count();
    if occurrences == 0 {
        return Err(ToolError::invalid(format!(
            "oldString was not found in {relative}. Read the file again and copy the exact text, including indentation."
        )));
    }
    if occurrences > 1 && !input.replace_all {
        return Err(ToolError::invalid(format!(
            "oldString appears {occurrences} times in {relative}. Add surrounding context or set replaceAll=true."
        )));
    }

    let updated = if input.replace_all {
        original.replace(&input.old_string, &input.new_string)
    } else {
        original.replacen(&input.old_string, &input.new_string, 1)
    };

    tokio::task::spawn_blocking({
        let path = path.clone();
        let updated = updated.clone();
        move || oxidecode_core::util::atomic_write(&path, updated.as_bytes())
    })
    .await
    .map_err(|error| ToolError::failed(error.to_string()))?
    .map_err(|error| ToolError::failed(error.to_string()))?;

    let patch = unified_diff(&relative, &original, &updated);
    let additions = patch.lines().filter(|line| line.starts_with('+') && !line.starts_with("+++")).count();
    let deletions = patch.lines().filter(|line| line.starts_with('-') && !line.starts_with("---")).count();

    Ok(ToolOutput::new(
        format!("Edited {relative}"),
        format!(
            "Edited {relative}: {additions} addition(s), {deletions} deletion(s).\n\n{patch}"
        ),
        json!({
            "filePath": relative,
            "additions": additions,
            "deletions": deletions,
            "patch": patch,
            "replacements": if input.replace_all { occurrences } else { 1 },
        }),
    ))
}
