use super::context::{ToolContext, ToolError, ToolOutput};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

fn definition_of(
    name: &str,
    description: &str,
    parameters: Value,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        definition_of(
            "bash",
            "Executes a shell command in a persistent session. Use for builds, tests, git and any command-line work. Prefer this over guessing. Output is truncated to the last 2000 lines. Avoid commands that need interactive input.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run."},
                    "timeout": {"type": "number", "description": "Optional timeout in milliseconds (max 600000)."},
                    "workdir": {"type": "string", "description": "Working directory relative to the project root."},
                    "description": {"type": "string", "description": "Short human description of the command (5-10 words)."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "edit",
            "Performs an exact string replacement in a file. oldString must match the file exactly, including indentation. If oldString appears more than once, provide more context or set replaceAll. Always read the file first.",
            json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Path to the file, relative to the project root."},
                    "oldString": {"type": "string", "description": "Exact text to replace."},
                    "newString": {"type": "string", "description": "Replacement text."},
                    "replaceAll": {"type": "boolean", "description": "Replace every occurrence."}
                },
                "required": ["filePath", "oldString", "newString"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "glob",
            "Finds files by glob pattern (for example `**/*.rs` or `src/**/*.ts`) relative to the project root. Returns paths sorted by modification time.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, e.g. `**/*.rs`."},
                    "path": {"type": "string", "description": "Directory to search (defaults to the project root)."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "grep",
            "Searches file contents with a regular expression. Respects .gitignore. Use `include` to filter file names, e.g. `*.rs`.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression to search for."},
                    "path": {"type": "string", "description": "File or directory to search."},
                    "include": {"type": "string", "description": "Glob filter for file names."},
                    "literal": {"type": "boolean", "description": "Treat the pattern as literal text."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "list",
            "Lists files and directories in a folder. Directories end with `/` and symlinks with `@`.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory to list (defaults to the project root)."}
                },
                "additionalProperties": false
            }),
        ),
        definition_of(
            "read",
            "Reads a file with line numbers. Reads up to 2000 lines by default; use offset and limit for larger files. Always read a file before editing it.",
            json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Path to the file, relative to the project root."},
                    "offset": {"type": "number", "description": "1-based line offset to start from, or 0 for the beginning."},
                    "limit": {"type": "number", "description": "Maximum number of lines to read."}
                },
                "required": ["filePath"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "write",
            "Writes (or overwrites) a file with the given content. Creates parent directories. Use edit for partial changes.",
            json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Path to the file, relative to the project root."},
                    "content": {"type": "string", "description": "Full file content."}
                },
                "required": ["filePath", "content"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "webfetch",
            "Fetches a URL and returns it as markdown (default), text, or html. Useful for documentation and web search results.",
            json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to fetch."},
                    "format": {"type": "string", "enum": ["markdown", "text", "html"], "description": "Output format."},
                    "timeout": {"type": "number", "description": "Timeout in seconds (max 120)."}
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "todowrite",
            "Creates and updates the session todo list. Use it to plan multi-step work and keep the user informed. Each todo has content, status (pending, in_progress, completed, cancelled) and optional priority (high, medium, low).",
            json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]},
                                "priority": {"type": "string", "enum": ["high", "medium", "low"]},
                                "id": {"type": "string"}
                            },
                            "required": ["content", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false
            }),
        ),
        definition_of(
            "todoread",
            "Reads the current session todo list.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
    ]
}

pub fn definition(name: &str) -> Option<ToolDefinition> {
    definitions().into_iter().find(|tool| tool.name == name)
}

pub async fn execute(
    name: &str,
    context: &ToolContext,
    input: Value,
) -> Result<ToolOutput, ToolError> {
    match name {
        "bash" => {
            let input: super::bash::Input = parse(name, input)?;
            super::bash::run(context, input).await
        }
        "edit" => {
            let input: super::edit::Input = parse(name, input)?;
            super::edit::run(context, input).await
        }
        "glob" => {
            let input: super::glob::Input = parse(name, input)?;
            super::glob::run(context, input).await
        }
        "grep" => {
            let input: super::grep::Input = parse(name, input)?;
            super::grep::run(context, input).await
        }
        "list" => {
            let input: super::list::Input = parse(name, input)?;
            super::list::run(context, input).await
        }
        "read" => {
            let input: super::read::Input = parse(name, input)?;
            super::read::run(context, input).await
        }
        "write" => {
            let input: super::write::Input = parse(name, input)?;
            super::write::run(context, input).await
        }
        "webfetch" => {
            let input: super::webfetch::Input = parse(name, input)?;
            super::webfetch::run(context, input).await
        }
        "todowrite" => {
            let input: super::todo::WriteInput = parse(name, input)?;
            super::todo::write(context, input).await
        }
        "todoread" => super::todo::read(context).await,
        other => Err(ToolError::invalid(format!("unknown tool `{other}`"))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(name: &str, value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value)
        .map_err(|error| ToolError::invalid(format!("invalid input for {name}: {error}")))
}
