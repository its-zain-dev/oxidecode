use oxidecode_core::config::LoadedConfig;
use oxidecode_core::event::EventBus;
use oxidecode_core::store::Store;
use oxidecode_core::util;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use crate::path;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Failed(String),
}

impl ToolError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::Denied(message.into())
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// One-line summary shown in the tool card header.
    pub title: String,
    /// The body returned to the model.
    pub output: String,
    /// Structured metadata used by the UI (diffs, counts, exit codes).
    #[serde(default)]
    pub metadata: Value,
}

impl ToolOutput {
    pub fn text(output: impl Into<String>) -> Self {
        let output = output.into();
        let title = output.lines().next().unwrap_or_default().to_string();
        Self {
            title,
            output,
            metadata: Value::Null,
        }
    }

    pub fn new(title: impl Into<String>, output: impl Into<String>, metadata: Value) -> Self {
        Self {
            title: title.into(),
            output: output.into(),
            metadata,
        }
    }
}

/// A permission request as emitted over SSE (`permission.asked`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub action: String,
    pub resources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub save: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Ask,
    Deny,
}

/// Decides whether a tool may run. The server supplies an implementation that
/// consults the session permission rules and asks the user when needed.
#[async_trait::async_trait]
pub trait PermissionPolicy: Send + Sync {
    /// Rules-only evaluation (no user interaction).
    async fn evaluate(&self, request: &PermissionRequest) -> Approval;

    /// Blocks until the user answers a permission prompt.
    async fn ask(&self, request: &PermissionRequest) -> bool;
}

/// Policy that allows everything (used by `--auto` and tests).
pub struct AllowAll;

#[async_trait::async_trait]
impl PermissionPolicy for AllowAll {
    async fn evaluate(&self, _request: &PermissionRequest) -> Approval {
        Approval::Allow
    }

    async fn ask(&self, _request: &PermissionRequest) -> bool {
        true
    }
}

/// Policy that denies everything (used by read-only agents).
pub struct DenyAll;

#[async_trait::async_trait]
impl PermissionPolicy for DenyAll {
    async fn evaluate(&self, _request: &PermissionRequest) -> Approval {
        Approval::Deny
    }

    async fn ask(&self, _request: &PermissionRequest) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct ToolContext {
    /// The working directory the agent was started in (the session location).
    pub directory: PathBuf,
    pub session_id: String,
    pub message_id: String,
    pub agent: String,
    pub store: Store,
    pub bus: EventBus,
    pub config: Arc<LoadedConfig>,
    pub policy: Arc<dyn PermissionPolicy>,
    /// Aborted when the user interrupts the turn.
    pub abort: tokio::sync::watch::Receiver<bool>,
}

impl ToolContext {
    /// Canonicalises and confines a user-supplied path to the session directory.
    ///
    /// Absolute paths inside the project are accepted; `../` escapes are not.
    pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
        path::resolve(&self.directory, input)
    }

    /// Resolves a path without requiring it to exist yet (for writes).
    pub fn resolve_for_write(&self, input: &str) -> Result<PathBuf, ToolError> {
        path::resolve_for_write(&self.directory, input)
    }

    pub fn relative(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.directory)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    pub async fn authorize(&self, request: PermissionRequest) -> Result<(), ToolError> {
        match self.policy.evaluate(&request).await {
            Approval::Allow => Ok(()),
            Approval::Deny => Err(ToolError::denied(format!(
                "permission denied for {}",
                request.action
            ))),
            Approval::Ask => {
                if self.policy.ask(&request).await {
                    Ok(())
                } else {
                    Err(ToolError::denied(format!(
                        "permission denied for {}",
                        request.action
                    )))
                }
            }
        }
    }

    pub fn aborted(&self) -> bool {
        *self.abort.borrow()
    }

    /// Emits a tool-scoped metadata update (used for live diffs in the TUI).
    pub fn emit_metadata(&self, name: &str, metadata: Value) {
        self.bus.emit_session(
            "session.tool.metadata",
            &self.directory.display().to_string(),
            serde_json::json!({
                "sessionID": self.session_id,
                "messageID": self.message_id,
                "tool": name,
                "metadata": metadata,
            }),
        );
    }

    pub fn truncate(&self, text: &str) -> (String, bool) {
        util::truncate_output(
            text,
            self.config.config.tool_output.max_lines,
            self.config.config.tool_output.max_bytes,
        )
    }
}
