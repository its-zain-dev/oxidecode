//! OxideCode core: configuration, paths, sessions, messages, storage, events.
//!
//! The crate is deliberately synchronous for storage (SQLite is memory-fast)
//! and async only where the event bus is involved.

pub mod auth;
pub mod config;
pub mod db;
pub mod event;
pub mod ids;
pub mod jsonc;
pub mod paths;
pub mod project;
pub mod session;
pub mod store;
pub mod util;
pub mod vcs;

pub use config::Config;
pub use db::Db;
pub use event::{Event, EventBus};
pub use paths::Paths;
pub use session::{
    AssistantMessage, AssistantPart, LocationRef, Message, ModelRef, SessionInfo, TokenUsage,
};
pub use store::Store;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The date OpenCode V2 shipped the API surface this clone targets.
pub const API_VERSION: &str = "2.2.6";
