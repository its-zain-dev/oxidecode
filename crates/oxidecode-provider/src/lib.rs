pub mod anthropic;
pub mod bun;
pub mod catalog;
pub mod client;
pub mod mock;
pub mod openai;
pub mod opencode_run;
pub mod types;

pub use catalog::{default_model, resolve, ModelInfo, ProviderInfo, ResolvedModel};
pub use client::ProviderClient;
pub use types::{ProviderEvent, ProviderRequest, ProviderStream, ToolSpec};
