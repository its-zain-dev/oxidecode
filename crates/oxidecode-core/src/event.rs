use crate::ids;
use crate::session::LocationRef;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

/// OpenCode V2 SSE event envelope. Every event carries an id, a creation time,
/// a dotted type name and a data payload; `location` is present on most runtime
/// events and is used by clients to scope updates to a project directory.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: String,
    pub created: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationRef>,
    pub data: Value,
}

impl Event {
    pub fn new(kind: impl Into<String>, location: Option<LocationRef>, data: Value) -> Self {
        Self {
            id: ids::event(),
            created: ids::now_ms(),
            kind: kind.into(),
            location,
            data,
        }
    }

    pub fn session(kind: impl Into<String>, directory: &str, data: Value) -> Self {
        Self::new(kind, Some(LocationRef::new(directory)), data)
    }
}

#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(16));
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: Event) {
        // A send error only means there are currently no subscribers.
        let _ = self.sender.send(event);
    }

    pub fn emit(&self, kind: impl Into<String>, location: Option<LocationRef>, data: Value) {
        self.publish(Event::new(kind, location, data));
    }

    pub fn emit_session(&self, kind: impl Into<String>, directory: &str, data: Value) {
        self.publish(Event::session(kind, directory, data));
    }

    pub fn connected(&self) -> Event {
        Event::new("server.connected", None, json!({}))
    }
}
