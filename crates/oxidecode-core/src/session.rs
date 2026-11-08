use crate::ids;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocationRef {
    pub directory: String,
}

impl LocationRef {
    pub fn new(directory: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl ModelRef {
    pub fn new(provider_id: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider_id: provider_id.into(),
            variant: None,
        }
    }

    pub fn display(&self) -> String {
        match &self.variant {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}#{}", self.provider_id, self.id, variant)
            }
            _ => format!("{}/{}", self.provider_id, self.id),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheUsage {
    pub read: u64,
    pub write: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache: CacheUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionTime {
    pub created: i64,
    pub updated: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub tokens: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub time: SessionTime,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub location: LocationRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

impl SessionInfo {
    pub fn new(
        project_id: impl Into<String>,
        directory: impl Into<String>,
        title: impl Into<String>,
        agent: Option<String>,
        model: Option<ModelRef>,
    ) -> Self {
        let now = ids::now_ms();
        Self {
            id: ids::session(),
            parent_id: None,
            project_id: project_id.into(),
            agent,
            model,
            cost: 0.0,
            tokens: TokenUsage::default(),
            outcome: None,
            time: SessionTime {
                created: now,
                updated: now,
                ..Default::default()
            },
            title: title.into(),
            location: LocationRef::new(directory),
            metadata: None,
            permissions: None,
            subpath: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageTime {
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streamed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<i64>,
}

impl MessageTime {
    pub fn now() -> Self {
        Self {
            created: ids::now_ms(),
            streamed: None,
            completed: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
    #[serde(rename = "retryAfterMs", default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl StructuredError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            status: None,
            response: None,
            retry_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantPart {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native: Option<Value>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<Value>,
    },
    #[serde(rename = "tool")]
    Tool(ToolPart),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTime {
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ran: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPart {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub executed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Value>,
    pub state: ToolState,
    pub time: ToolTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum ToolState {
    #[serde(rename = "streaming")]
    Streaming { input: String },
    #[serde(rename = "running")]
    Running { input: Value, metadata: Value },
    #[serde(rename = "completed")]
    Completed {
        input: Value,
        content: Vec<Value>,
        #[serde(default = "empty_object")]
        metadata: Value,
    },
    #[serde(rename = "error")]
    Error {
        input: Value,
        error: StructuredError,
        #[serde(default)]
        content: Vec<Value>,
        #[serde(default = "empty_object")]
        metadata: Value,
    },
}

fn empty_object() -> Value {
    json!({})
}

impl ToolPart {
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: String) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            executed: false,
            native: None,
            state: ToolState::Streaming { input },
            time: ToolTime {
                created: ids::now_ms(),
                ran: None,
                completed: None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub agent: String,
    pub model: ModelRef,
    pub content: Vec<AssistantPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(rename = "rawFinish", default, skip_serializing_if = "Option::is_none")]
    pub raw_finish: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Value>,
}

impl AssistantMessage {
    pub fn new(agent: impl Into<String>, model: ModelRef) -> Self {
        Self {
            id: ids::message(),
            metadata: None,
            time: MessageTime::now(),
            agent: agent.into(),
            model,
            content: Vec::new(),
            snapshot: None,
            finish: None,
            raw_finish: None,
            native: None,
            cost: None,
            tokens: None,
            error: None,
            retry: None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut String> {
        self.content.iter_mut().find_map(|part| match part {
            AssistantPart::Text { text, .. } => Some(text),
            _ => None,
        })
    }

    pub fn push_text(&mut self, delta: &str) {
        if let Some(last) = self.content.last_mut() {
            if let AssistantPart::Text { text, .. } = last {
                text.push_str(delta);
                return;
            }
        }
        self.content.push(AssistantPart::Text {
            text: delta.to_string(),
            native: None,
        });
    }

    pub fn push_reasoning(&mut self, delta: &str) {
        if let Some(last) = self.content.last_mut() {
            if let AssistantPart::Reasoning { text, .. } = last {
                text.push_str(delta);
                return;
            }
        }
        self.content.push(AssistantPart::Reasoning {
            text: delta.to_string(),
            native: None,
            time: None,
        });
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for part in &self.content {
            if let AssistantPart::Text { text, .. } = part {
                out.push_str(text);
            }
        }
        out
    }

    pub fn reasoning(&self) -> String {
        let mut out = String::new();
        for part in &self.content {
            if let AssistantPart::Reasoning { text, .. } = part {
                out.push_str(text);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    #[serde(rename = "shellID")]
    pub shell_id: String,
    pub command: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub skill: String,
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub time: MessageTime,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "shell")]
    Shell(ShellMessage),
    #[serde(rename = "system")]
    System(SystemMessage),
    #[serde(rename = "synthetic")]
    Synthetic(SyntheticMessage),
    #[serde(rename = "skill")]
    Skill(SkillMessage),
    #[serde(rename = "idle")]
    Idle(IdleMessage),
    #[serde(rename = "agent-switched")]
    AgentSelected(Value),
    #[serde(rename = "model-switched")]
    ModelSelected(Value),
    #[serde(rename = "location-switched")]
    LocationSwitched(Value),
    #[serde(rename = "compaction")]
    Compaction(Value),
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self::User(UserMessage {
            id: ids::message(),
            metadata: None,
            time: MessageTime::now(),
            text: text.into(),
            files: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
        })
    }

    pub fn assistant(agent: impl Into<String>, model: ModelRef) -> Self {
        Self::Assistant(AssistantMessage::new(agent, model))
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::System(SystemMessage {
            id: ids::message(),
            metadata: None,
            time: MessageTime::now(),
            text: text.into(),
        })
    }

    pub fn idle(outcome: impl Into<String>) -> Self {
        Self::Idle(IdleMessage {
            id: ids::message(),
            metadata: None,
            time: MessageTime::now(),
            outcome: outcome.into(),
            error: None,
        })
    }

    pub fn id(&self) -> &str {
        match self {
            Message::User(message) => &message.id,
            Message::Assistant(message) => &message.id,
            Message::Shell(message) => &message.id,
            Message::System(message) => &message.id,
            Message::Synthetic(message) => &message.id,
            Message::Skill(message) => &message.id,
            Message::Idle(message) => &message.id,
            Message::AgentSelected(value)
            | Message::ModelSelected(value)
            | Message::LocationSwitched(value)
            | Message::Compaction(value) => value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::Shell(_) => "shell",
            Message::System(_) => "system",
            Message::Synthetic(_) => "synthetic",
            Message::Skill(_) => "skill",
            Message::Idle(_) => "idle",
            Message::AgentSelected(_) => "agent-switched",
            Message::ModelSelected(_) => "model-switched",
            Message::LocationSwitched(_) => "location-switched",
            Message::Compaction(_) => "compaction",
        }
    }

    pub fn created(&self) -> i64 {
        match self {
            Message::User(message) => message.time.created,
            Message::Assistant(message) => message.time.created,
            Message::Shell(message) => message.time.created,
            Message::System(message) => message.time.created,
            Message::Synthetic(message) => message.time.created,
            Message::Skill(message) => message.time.created,
            Message::Idle(message) => message.time.created,
            Message::AgentSelected(value)
            | Message::ModelSelected(value)
            | Message::LocationSwitched(value)
            | Message::Compaction(value) => value
                .get("time")
                .and_then(|time| time.get("created"))
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        }
    }

    pub fn as_assistant(&self) -> Option<&AssistantMessage> {
        match self {
            Message::Assistant(message) => Some(message),
            _ => None,
        }
    }

    pub fn as_assistant_mut(&mut self) -> Option<&mut AssistantMessage> {
        match self {
            Message::Assistant(message) => Some(message),
            _ => None,
        }
    }
}

/// One chat turn handed to a provider, after reconstructing session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Vec<ChatPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { mime: String, data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
