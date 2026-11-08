use oxidecode_core::session::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use futures_util::Stream;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum ProviderEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolInputStarted { id: String, name: String },
    ToolInputDelta { id: String, delta: String },
    ToolInputEnded { id: String, name: String, input: Value },
    Usage {
        input: u64,
        output: u64,
        reasoning: u64,
        cache_read: u64,
        cache_write: u64,
    },
    Finish { reason: String },
    Error { message: String, status: Option<u16> },
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, anyhow::Error>> + Send>>;
