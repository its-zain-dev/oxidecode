use crate::catalog::ResolvedModel;
use crate::client::{base_headers, SseDecoder};
use crate::types::{ProviderEvent, ProviderRequest, ProviderStream};
use anyhow::{bail, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};

/// Anthropic Messages API streaming client.
///
/// The wire format differs from OpenAI in three ways that matter here:
///   * the system prompt is a top-level field, not a message;
///   * tool calls arrive as `content_block_start` / `input_json_delta` pairs;
///   * usage is reported on `message_start` and `message_delta`.
pub async fn stream(
    http: &reqwest::Client,
    model: &ResolvedModel,
    request: ProviderRequest,
) -> Result<ProviderStream> {
    let url = format!("{}/messages", model.base_url.trim_end_matches('/'));
    let mut body = json!({
        "model": request.model_id,
        "max_tokens": request.max_tokens.unwrap_or(8192),
        "system": request.system,
        "messages": to_anthropic_messages(&request),
        "stream": true,
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    })
                })
                .collect(),
        );
    }
    if let Some(temperature) = request.temperature {
        // Anthropic caps temperature at 1.0.
        body["temperature"] = json!(temperature.clamp(0.0, 1.0));
    }

    let mut builder = http
        .post(&url)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if let Some(key) = &model.api_key {
        builder = builder.header("x-api-key", key);
    }
    let builder = base_headers(builder, model);
    let response = builder.send().await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or(text);
        bail!("anthropic returned HTTP {}: {}", status.as_u16(), truncate(&detail, 1200));
    }

    let mut byte_stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut blocks: Vec<Block> = Vec::new();
    let mut finished = false;

    let stream = async_stream::stream! {
        'outer: while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Ok(ProviderEvent::Error { message: error.to_string(), status: None });
                    return;
                }
            };
            for payload in decoder.push(&chunk) {
                let value: Value = match serde_json::from_str(&payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "message_start" => {
                        if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                            let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                            let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                            let cache_read = usage
                                .get("cache_read_input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let cache_write = usage
                                .get("cache_creation_input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            if input > 0 || output > 0 {
                                yield Ok(ProviderEvent::Usage {
                                    input,
                                    output,
                                    reasoning: 0,
                                    cache_read,
                                    cache_write,
                                });
                            }
                        }
                    }
                    "content_block_start" => {
                        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let block = value.get("content_block").cloned().unwrap_or(Value::Null);
                        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                        let entry = if kind == "tool_use" {
                            let id = block
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("tool_{index}"));
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if !name.is_empty() {
                                yield Ok(ProviderEvent::ToolInputStarted {
                                    id: id.clone(),
                                    name: name.clone(),
                                });
                            }
                            Block::Tool { id, name, arguments: String::new() }
                        } else if kind == "thinking" {
                            Block::Thinking
                        } else {
                            Block::Text
                        };
                        if blocks.len() <= index {
                            blocks.resize(index + 1, Block::Text);
                        }
                        blocks[index] = entry;
                    }
                    "content_block_delta" => {
                        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let delta = value.get("delta").cloned().unwrap_or(Value::Null);
                        let delta_kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
                        match delta_kind {
                            "text_delta" => {
                                if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        yield Ok(ProviderEvent::TextDelta(text.to_string()));
                                    }
                                }
                            }
                            "thinking_delta" => {
                                if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        yield Ok(ProviderEvent::ReasoningDelta(text.to_string()));
                                    }
                                }
                            }
                            "input_json_delta" => {
                                if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                                    if let Some(Block::Tool { id, arguments, .. }) = blocks.get_mut(index) {
                                        arguments.push_str(partial);
                                        yield Ok(ProviderEvent::ToolInputDelta {
                                            id: id.clone(),
                                            delta: partial.to_string(),
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        if let Some(Block::Tool { id, name, arguments }) = blocks.get_mut(index) {
                            let input = serde_json::from_str::<Value>(arguments)
                                .unwrap_or_else(|_| json!({ "_raw": arguments }));
                            let event = ProviderEvent::ToolInputEnded {
                                id: id.clone(),
                                name: name.clone(),
                                input,
                            };
                            *arguments = String::new();
                            yield Ok(event);
                        }
                    }
                    "message_delta" => {
                        if let Some(usage) = value.get("usage") {
                            let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                            if output > 0 {
                                yield Ok(ProviderEvent::Usage {
                                    input: 0,
                                    output,
                                    reasoning: 0,
                                    cache_read: 0,
                                    cache_write: 0,
                                });
                            }
                        }
                        if let Some(reason) = value
                            .get("delta")
                            .and_then(|delta| delta.get("stop_reason"))
                            .and_then(Value::as_str)
                        {
                            if !finished {
                                finished = true;
                                yield Ok(ProviderEvent::Finish { reason: map_stop(reason) });
                            }
                        }
                    }
                    "message_stop" => {
                        if !finished {
                            yield Ok(ProviderEvent::Finish { reason: "stop".into() });
                        }
                        break 'outer;
                    }
                    "error" => {
                        let message = value
                            .get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("anthropic stream error")
                            .to_string();
                        yield Ok(ProviderEvent::Error { message, status: None });
                        break 'outer;
                    }
                    _ => {}
                }
            }
        }
    };

    Ok(Box::pin(stream))
}

#[derive(Clone)]
enum Block {
    Text,
    Thinking,
    Tool { id: String, name: String, arguments: String },
}

fn map_stop(reason: &str) -> String {
    match reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool-calls",
        _ => "unknown",
    }
    .to_string()
}

fn to_anthropic_messages(request: &ProviderRequest) -> Vec<Value> {
    // Tool results must be wrapped in a user message with `tool_result` blocks,
    // and consecutive messages of the same role are merged.
    let mut out: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role.as_str() {
            "assistant" => {
                let mut content: Vec<Value> = Vec::new();
                for part in &message.content {
                    if let oxidecode_core::session::ChatPart::Text { text } = part {
                        if !text.is_empty() {
                            content.push(json!({"type": "text", "text": text}));
                        }
                    }
                }
                for call in &message.tool_calls {
                    let input = serde_json::from_str::<Value>(&call.arguments)
                        .unwrap_or_else(|_| json!({}));
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": input,
                    }));
                }
                if content.is_empty() {
                    continue;
                }
                push_role(&mut out, "assistant", content);
            }
            "tool" => {
                let text = message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        oxidecode_core::session::ChatPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": text,
                });
                push_role(&mut out, "user", vec![block]);
            }
            _ => {
                let mut content: Vec<Value> = Vec::new();
                for part in &message.content {
                    match part {
                        oxidecode_core::session::ChatPart::Text { text } => {
                            if !text.is_empty() {
                                content.push(json!({"type": "text", "text": text}));
                            }
                        }
                        oxidecode_core::session::ChatPart::Image { mime, data } => {
                            content.push(json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": mime,
                                    "data": data,
                                }
                            }));
                        }
                    }
                }
                if content.is_empty() {
                    continue;
                }
                push_role(&mut out, "user", content);
            }
        }
    }
    out
}

fn push_role(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.extend(blocks);
                return;
            }
        }
    }
    out.push(json!({"role": role, "content": blocks}));
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}
