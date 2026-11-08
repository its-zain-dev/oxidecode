use super::context::{ToolContext, ToolError, ToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct WriteInput {
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

const VALID_STATUS: [&str; 4] = ["pending", "in_progress", "completed", "cancelled"];

pub async fn write(context: &ToolContext, input: WriteInput) -> Result<ToolOutput, ToolError> {
    if input.todos.len() > 200 {
        return Err(ToolError::invalid("too many todos (max 200)"));
    }
    for todo in &input.todos {
        if !VALID_STATUS.contains(&todo.status.as_str()) {
            return Err(ToolError::invalid(format!(
                "invalid status `{}`; expected one of {VALID_STATUS:?}",
                todo.status
            )));
        }
        if todo.content.trim().is_empty() {
            return Err(ToolError::invalid("todo content cannot be empty"));
        }
    }

    let values: Vec<Value> = input
        .todos
        .iter()
        .enumerate()
        .map(|(index, todo)| {
            json!({
                "id": todo.id.clone().unwrap_or_else(|| format!("todo-{}", index + 1)),
                "content": todo.content,
                "status": todo.status,
                "priority": todo.priority.clone().unwrap_or_else(|| "medium".to_string()),
            })
        })
        .collect();
    context
        .store
        .replace_todos(&context.session_id, &values)
        .map_err(|error| ToolError::failed(error.to_string()))?;
    context.bus.emit_session(
        "session.todo.updated",
        &context.directory.display().to_string(),
        json!({ "sessionID": context.session_id, "todos": values }),
    );

    let completed = input
        .todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .count();
    let summary = input
        .todos
        .iter()
        .map(|todo| {
            let marker = match todo.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[~]",
                "cancelled" => "[-]",
                _ => "[ ]",
            };
            format!("{marker} {}", todo.content)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ToolOutput::new(
        format!(
            "Updated todos ({completed}/{} completed)",
            input.todos.len()
        ),
        if summary.is_empty() {
            "Todo list cleared.".to_string()
        } else {
            summary
        },
        json!({ "count": input.todos.len(), "completed": completed }),
    ))
}

pub async fn read(context: &ToolContext) -> Result<ToolOutput, ToolError> {
    let todos = context
        .store
        .list_todos(&context.session_id)
        .map_err(|error| ToolError::failed(error.to_string()))?;
    let body = serde_json::to_string_pretty(&todos).unwrap_or_else(|_| "[]".to_string());
    Ok(ToolOutput::new(
        format!("{} todo(s)", todos.len()),
        body,
        json!({ "count": todos.len() }),
    ))
}
