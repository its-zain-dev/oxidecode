use crate::catalog::{Protocol, ResolvedModel};
use crate::types::{ProviderRequest, ProviderStream};
use anyhow::Result;
use bytes::BytesMut;
use std::time::Duration;

/// One shared HTTP client for every provider request.
///
/// The engine is deliberately request/response only: no helper process, no
/// JavaScript runtime and no temporary request dumps. Connection pooling and a
/// bounded idle pool keep memory use predictable on mobile devices.
pub struct ProviderClient {
    http: reqwest::Client,
}

impl ProviderClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .user_agent(concat!("oxidecode/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }

    pub async fn stream(
        &self,
        model: &ResolvedModel,
        request: ProviderRequest,
    ) -> Result<ProviderStream> {
        match model.protocol {
            Protocol::OpenAi => crate::openai::stream(&self.http, model, request).await,
            Protocol::Anthropic => crate::anthropic::stream(&self.http, model, request).await,
            Protocol::Mock => Ok(crate::mock::stream(request)),
        }
    }
}

impl Default for ProviderClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental Server-Sent Events decoder. Handles `\n\n`, `\r\n\r\n`, and mixed
/// line separators, and returns each `data:` payload already joined.
#[derive(Default)]
pub struct SseDecoder {
    buffer: BytesMut,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        loop {
            let Some((end, separator)) = find_separator(&self.buffer) else {
                break;
            };
            let frame = self.buffer.split_to(end).freeze();
            let _ = self.buffer.split_to(separator);
            if let Some(data) = extract_frame_data(&frame) {
                events.push(data);
            }
        }
        events
    }

    /// Flushes any trailing SSE frame left in the buffer when the byte stream closes.
    pub fn finish(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let frame = self.buffer.split().freeze();
        extract_frame_data(&frame).into_iter().collect()
    }
}

fn extract_frame_data(frame: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(frame);
    let mut data = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("data:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

fn find_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        if buffer[index..].starts_with(b"\r\n\n") || buffer[index..].starts_with(b"\n\r\n") {
            return Some((index, 3));
        }
    }
    None
}

pub fn base_headers(
    builder: reqwest::RequestBuilder,
    model: &ResolvedModel,
) -> reqwest::RequestBuilder {
    let mut builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
    for (key, value) in &model.headers {
        builder = builder.header(key, value);
    }
    builder
}

/// Safe UTF-8 character truncation for error messages.
pub fn truncate_utf8(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Formats an HTTP error body from a provider, preserving status codes and
/// adding actionable guidance for quota and authentication responses.
pub fn format_http_error(provider_label: &str, status: u16, body: &str, is_free: bool) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let error_type = parsed
        .as_ref()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    e.get("type")
                        .or_else(|| e.get("code"))
                        .or_else(|| e.get("_tag"))
                })
                .or_else(|| v.get("type"))
                .or_else(|| v.get("_tag"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let message = parsed
        .as_ref()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| e.as_str())
                })
                .or_else(|| v.get("message").and_then(serde_json::Value::as_str))
                .or_else(|| v.get("errorMessage").and_then(serde_json::Value::as_str))
        })
        .unwrap_or(body)
        .trim();

    let detail = if !error_type.is_empty() && !message.contains(error_type) {
        format!("{error_type}: {}", truncate_utf8(message, 1200))
    } else {
        truncate_utf8(message, 1200)
    };

    if status == 429 {
        return format!(
            "{provider_label} returned HTTP 429 ({detail}). The provider is rate limiting requests; wait for the quota window to reset or configure another provider key."
        );
    }

    if status == 403
        && (error_type == "FreeTierError"
            || detail.contains("FreeTierError")
            || detail.contains("can only be used from within OpenCode"))
    {
        return format!(
            "{provider_label} returned HTTP 403 ({detail}). Configure OPENCODE_API_KEY (or ~/.local/share/opencode/auth.json) with an active account, or configure another provider key (e.g. DEEPSEEK_API_KEY, GEMINI_API_KEY, OPENROUTER_API_KEY, GROQ_API_KEY, OPENAI_API_KEY, ANTHROPIC_API_KEY)."
        );
    }

    if status == 401 && is_free {
        return format!(
            "{provider_label} returned HTTP 401 ({detail}). Export OPENCODE_API_KEY or configure a provider key in ~/.config/opencode/opencode.json or ~/.local/share/opencode/auth.json."
        );
    }

    format!("{provider_label} returned HTTP {status}: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_mixed_line_endings() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"data: one\r\n\r\ndata: two\n\ndata: three\r\n\n");
        assert_eq!(events, vec!["one", "two", "three"]);
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn joins_multiline_data_and_flushes_tail() {
        let mut decoder = SseDecoder::default();
        assert_eq!(decoder.push(b"data: a\ndata: b\n\n"), vec!["a\nb"]);
        let mut tail = SseDecoder::default();
        tail.push(b"data: last");
        assert_eq!(tail.finish(), vec!["last"]);
    }
}
