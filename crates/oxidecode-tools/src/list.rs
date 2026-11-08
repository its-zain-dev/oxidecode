use super::context::{ToolContext, ToolError, ToolOutput};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub path: Option<String>,
}

const MAX_ENTRIES: usize = 1000;

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let path = match input.path.as_deref() {
        Some(value) if !value.trim().is_empty() => context.resolve(value)?,
        _ => context.directory.clone(),
    };
    if !path.is_dir() {
        return Err(ToolError::invalid(format!(
            "{} is not a directory",
            context.relative(&path)
        )));
    }

    let mut entries: Vec<(bool, String)> = Vec::new();
    let mut reader = tokio::fs::read_dir(&path)
        .await
        .map_err(|error| ToolError::failed(error.to_string()))?;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| ToolError::failed(error.to_string()))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let suffix = if file_type.is_dir() {
            "/"
        } else if file_type.is_symlink() {
            "@"
        } else {
            ""
        };
        entries.push((file_type.is_dir(), format!("{name}{suffix}")));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);

    let relative = context.relative(&path);
    let listing = entries
        .iter()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut output = if listing.is_empty() {
        format!("{relative} is empty.")
    } else {
        format!("{relative}:\n{listing}\n")
    };
    if total > MAX_ENTRIES {
        output.push_str(&format!("\n[{} more entries omitted.]\n", total - MAX_ENTRIES));
    }
    Ok(ToolOutput::new(
        format!("Listed {relative} ({total} entries)"),
        output,
        json!({ "path": relative, "count": total }),
    ))
}
