//! Thin synchronous HTTP client for the OxideCode/OpenCode V2 API.
//!
//! The TUI runs on the main thread and the SSE stream runs on a background
//! thread, so a blocking client keeps the design simple and avoids a second
//! async runtime competing with the server.

use base64::Engine;
use oxidecode_core::session::{Message, SessionInfo};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum UiEvent {
    Event(Value),
    Connected,
    Disconnected(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimit {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "providerID", default)]
    pub provider_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub limit: ModelLimit,
    #[serde(default)]
    pub cost: Value,
    #[serde(default)]
    pub variants: Value,
    #[serde(default)]
    pub capabilities: Value,
}

impl ModelInfo {
    pub fn display(&self) -> String {
        format!("{}/{}", self.provider_id, self.id)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub time: SessionTime,
    #[serde(default)]
    pub model: Option<oxidecode_core::session::ModelRef>,
    #[serde(default)]
    pub cost: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionTime {
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub updated: i64,
}

#[derive(Clone)]
pub struct Client {
    pub base: String,
    pub password: String,
    pub directory: String,
    http: reqwest::blocking::Client,
    stream: reqwest::blocking::Client,
}

impl Client {
    pub fn new(base: impl Into<String>, password: impl Into<String>, directory: impl Into<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(8)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        let stream = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(24 * 60 * 60))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            password: password.into(),
            directory: directory.into(),
            http,
            stream,
        }
    }

    fn auth(&self) -> String {
        let raw = format!("opencode:{}", self.password);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    fn request(
        &self,
        client: &reqwest::blocking::Client,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::blocking::RequestBuilder {
        client
            .request(method, format!("{}{}", self.base, path))
            .header(reqwest::header::AUTHORIZATION, self.auth())
            .header(reqwest::header::ACCEPT, "application/json")
    }

    fn with_directory(&self, mut builder: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        builder = builder.query(&[(
            "location[directory]",
            self.directory.clone(),
        )]);
        builder
    }

    pub fn get(&self, path: &str) -> anyhow::Result<Value> {
        let response = self
            .with_directory(self.request(&self.http, reqwest::Method::GET, path))
            .send()?;
        self.decode(response)
    }

    pub fn get_no_location(&self, path: &str) -> anyhow::Result<Value> {
        let response = self
            .request(&self.http, reqwest::Method::GET, path)
            .send()?;
        self.decode(response)
    }

    pub fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let response = self
            .with_directory(self.request(&self.http, reqwest::Method::POST, path))
            .json(&body)
            .send()?;
        self.decode_optional(response)
    }

    pub fn post_no_location(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let response = self
            .request(&self.http, reqwest::Method::POST, path)
            .json(&body)
            .send()?;
        self.decode_optional(response)
    }

    pub fn patch(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let response = self
            .with_directory(self.request(&self.http, reqwest::Method::PATCH, path))
            .json(&body)
            .send()?;
        self.decode_optional(response)
    }

    pub fn delete(&self, path: &str) -> anyhow::Result<Value> {
        let response = self
            .with_directory(self.request(&self.http, reqwest::Method::DELETE, path))
            .send()?;
        self.decode_optional(response)
    }

    fn decode(&self, response: reqwest::blocking::Response) -> anyhow::Result<Value> {
        match self.decode_optional(response)? {
            Value::Null => Ok(Value::Null),
            value => Ok(value),
        }
    }

    fn decode_optional(&self, response: reqwest::blocking::Response) -> anyhow::Result<Value> {
        let status = response.status();
        let bytes = response.bytes()?;
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes);
            let message = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .or_else(|| value.get("error"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| {
                    if text.is_empty() {
                        status.to_string()
                    } else {
                        text.to_string()
                    }
                });
            anyhow::bail!("{message}");
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let value = self.get("/api/model")?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data).unwrap_or_default())
    }

    pub fn agents(&self) -> anyhow::Result<Vec<AgentInfo>> {
        let value = self.get("/api/agent")?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data).unwrap_or_default())
    }

    pub fn sessions(&self, limit: usize) -> anyhow::Result<Vec<SessionSummary>> {
        let path = format!("/api/session?limit={limit}&order=desc");
        let value = self.get(&path)?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data).unwrap_or_default())
    }

    pub fn create_session(
        &self,
        title: Option<&str>,
        model: Option<&crate::api::ModelChoice>,
        agent: Option<&str>,
    ) -> anyhow::Result<SessionInfo> {
        let mut body = json!({ "location": { "directory": self.directory } });
        if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
            body["title"] = json!(title);
        }
        if let Some(model) = model {
            body["model"] = model.to_json();
        }
        if let Some(agent) = agent.filter(|value| !value.is_empty()) {
            body["agent"] = json!(agent);
        }
        let value = self.post_no_location("/api/session", body)?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data)?)
    }

    pub fn session(&self, id: &str) -> anyhow::Result<SessionInfo> {
        let value = self.get(&format!("/api/session/{id}"))?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data)?)
    }

    pub fn messages(&self, id: &str) -> anyhow::Result<Vec<Message>> {
        let value = self.get(&format!("/api/session/{id}/message?limit=5000&order=asc"))?;
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        Ok(serde_json::from_value(data).unwrap_or_default())
    }

    pub fn prompt(&self, id: &str, text: &str) -> anyhow::Result<Value> {
        self.post(
            &format!("/api/session/{id}/prompt"),
            json!({ "text": text, "delivery": "queue" }),
        )
    }

    pub fn switch_model(&self, id: &str, model: &ModelChoice) -> anyhow::Result<Value> {
        self.post(
            &format!("/api/session/{id}/model"),
            json!({ "model": model.to_json() }),
        )
    }

    pub fn switch_agent(&self, id: &str, agent: &str) -> anyhow::Result<Value> {
        self.post(&format!("/api/session/{id}/agent"), json!({ "agent": agent }))
    }

    pub fn interrupt(&self, id: &str) -> anyhow::Result<Value> {
        self.post(&format!("/api/session/{id}/interrupt"), json!({}))
    }

    pub fn compact(&self, id: &str) -> anyhow::Result<Value> {
        self.post(&format!("/api/session/{id}/compact"), json!({}))
    }

    pub fn reply_permission(&self, session: &str, request: &str, decision: &str) -> anyhow::Result<Value> {
        self.post(
            &format!("/api/session/{session}/permission/{request}/reply"),
            json!({ "decision": decision }),
        )
    }

    pub fn permissions(&self, session: &str) -> anyhow::Result<Vec<Value>> {
        let value = self.get(&format!("/api/session/{session}/permission"))?;
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub fn find_files(&self, query: &str) -> anyhow::Result<Vec<String>> {
        let path = format!(
            "/api/fs/find?query={}&limit=100&fuzzy=true&type=file",
            encode(query)
        );
        let value = self.get(&path)?;
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("path").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<Value> {
        self.delete(&format!("/api/session/{id}"))
    }

    /// Opens the event stream. The caller reads lines until the connection ends.
    pub fn events(&self) -> anyhow::Result<reqwest::blocking::Response> {
        let response = self
            .with_directory(self.request(&self.stream, reqwest::Method::GET, "/api/event"))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!("event stream failed with HTTP {}", response.status());
        }
        Ok(response)
    }
}

/// Streams server events into `sender`, reconnecting automatically.
pub fn spawn_events(client: Client, sender: Sender<UiEvent>) {
    std::thread::Builder::new()
        .name("oxidecode-sse".into())
        .spawn(move || {
            loop {
                match client.events() {
                    Ok(response) => {
                        if sender.send(UiEvent::Connected).is_err() {
                            return;
                        }
                        let mut reader = BufReader::new(response);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            match reader.read_line(&mut line) {
                                Ok(0) => break,
                                Ok(_) => {
                                    let trimmed = line.trim_end_matches(['\r', '\n']);
                                    let Some(payload) = trimmed.strip_prefix("data:") else {
                                        continue;
                                    };
                                    let payload = payload.trim_start();
                                    if payload.is_empty() {
                                        continue;
                                    }
                                    if let Ok(value) = serde_json::from_str::<Value>(payload) {
                                        if sender.send(UiEvent::Event(value)).is_err() {
                                            return;
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if sender
                            .send(UiEvent::Disconnected("connection closed".into()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if sender
                            .send(UiEvent::Disconnected(error.to_string()))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider_id: String,
    pub id: String,
    pub variant: Option<String>,
}

impl ModelChoice {
    pub fn new(provider_id: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            id: id.into(),
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

    pub fn to_json(&self) -> Value {
        let mut value = json!({ "providerID": self.provider_id, "id": self.id });
        if let Some(variant) = &self.variant {
            if !variant.is_empty() {
                value["variant"] = json!(variant);
            }
        }
        value
    }

    pub fn from_model(model: &ModelInfo) -> Self {
        Self::new(model.provider_id.clone(), model.id.clone())
    }
}

/// Percent-encodes a query value (RFC 3986 unreserved characters survive).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
