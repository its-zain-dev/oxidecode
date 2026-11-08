//! Agent tools for OxideCode.
//!
//! Each tool mirrors the input schema and output shape of the equivalent
//! OpenCode tool, so a client written against OpenCode (including the
//! TermuxFileManager Android app) sees familiar behaviour.

pub mod bash;
pub mod context;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod list;
pub mod path;
pub mod read;
pub mod registry;
pub mod todo;
pub mod webfetch;
pub mod write;

pub use context::{
    Approval, PermissionPolicy, PermissionRequest, ToolContext, ToolError, ToolOutput,
};
pub use registry::{definition, definitions, execute, ToolDefinition};
