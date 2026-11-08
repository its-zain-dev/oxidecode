use crate::catalog::{Protocol, ResolvedModel};
use crate::types::{ProviderRequest, ProviderStream};
use anyhow::Result;
use bytes::BytesMut;

pub struct ProviderClient {
    http: reqwest::Client,
}

impl ProviderClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(600))
            .tcp_nodelay(true)
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

/// Incremental Server-Sent Events decoder. Handles `\n\n` and `\r\n\r\n`
/// separators and returns each `data:` payload already joined.
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
            let text = String::from_utf8_lossy(&frame);
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
            if !data.is_empty() {
                events.push(data);
            }
        }
        events
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
    }
    None
}

pub fn base_headers(
    builder: reqwest::RequestBuilder,
    model: &ResolvedModel,
) -> reqwest::RequestBuilder {
    let mut builder = builder;
    for (key, value) in &model.headers {
        builder = builder.header(key, value);
    }
    builder
}
