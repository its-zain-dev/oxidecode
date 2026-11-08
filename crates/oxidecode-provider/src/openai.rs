use crate::catalog::ResolvedModel;
use crate::client::{base_headers, SseDecoder};
use crate::types::{ProviderEvent, ProviderRequest, ProviderStream};
use anyhow::{bail, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub async fn stream(
    http: &reqwest::Client,
    model: &ResolvedModel,
    request: ProviderRequest,
) -> Result<ProviderStream> {
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
    let mut body = json!({
        "model": request.model_id,
        "messages": to_openai_messages(&request),
        "stream": true,
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!("auto");
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    let supports_usage = ["openai", "openrouter", "groq", "deepseek", "opencode", "xai"]
        .contains(&model.provider_id.as_str());
    if supports_usage {
        body["stream_options"] = json!({"include_usage": true});
    }
    if model.provider_id == "openrouter" {
        body["provider"] = json!({"sort": "throughput"});
    }

    let mut builder = http.post(&url).json(&body);
    if let Some(key) = &model.api_key {
        builder = builder.bearer_auth(key);
    }
    if model.provider_id == "openrouter" {
        builder = builder
            .header("HTTP-Referer", "https://github.com/its-zain-dev/oxidecode")
            .header("X-Title", "OxideCode");
    }
    let builder = base_headers(builder, model);
    let response = builder.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or(body);
        bail!("provider returned HTTP {}: {}", status.as_u16(), truncate(&detail, 1200));
    }

    let mut byte_stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut partial_tools: BTreeMap<u64, PartialTool> = BTreeMap::new();
    let mut started: BTreeSet<u64> = BTreeSet::new();
    let mut ended: BTreeSet<u64> = BTreeSet::new();
    let mut finished = false;

    let stream = async_stream::stream! {
        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Ok(ProviderEvent::Error { message: error.to_string(), status: None });
                    return;
                }
            };
            for payload in decoder.push(&chunk) {
                if payload.trim() == "[DONE]" {
                    for event in finish_tools(&mut partial_tools, &mut ended) {
                        yield Ok(event);
                    }
                    if !finished {
                        yield Ok(ProviderEvent::Finish { reason: "stop".into() });
                    }
                    return;
                }
                let value: Value = match serde_json::from_str(&payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(error) = value.get("error") {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("provider stream error")
                        .to_string();
                    yield Ok(ProviderEvent::Error { message, status: None });
                    continue;
                }

                if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
                    let input = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let output = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let reasoning = usage
                        .get("completion_tokens_details")
                        .and_then(|details| details.get("reasoning_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("prompt_tokens_details")
                        .and_then(|details| details.get("cached_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_write = usage
                        .get("prompt_tokens_details")
                        .and_then(|details| details.get("cache_write_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    if input > 0 || output > 0 {
                        yield Ok(ProviderEvent::Usage {
                            input,
                            output,
                            reasoning,
                            cache_read,
                            cache_write,
                        });
                    }
                }

                let Some(choice) = value.get("choices").and_then(|choices| choices.get(0)) else {
                    continue;
                };
                let delta = choice.get("delta").unwrap_or(&Value::Null);

                if let Some(content) = delta.get("content") {
                    match content {
                        Value::String(text) if !text.is_empty() => {
                            yield Ok(ProviderEvent::TextDelta(text.clone()));
                        }
                        Value::Array(parts) => {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        yield Ok(ProviderEvent::TextDelta(text.to_string()));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }

                for field in ["reasoning_content", "reasoning"] {
                    if let Some(reasoning) = delta.get(field) {
                        match reasoning {
                            Value::String(text) if !text.is_empty() => {
                                yield Ok(ProviderEvent::ReasoningDelta(text.clone()));
                            }
                            Value::Object(map) => {
                                if let Some(text) = map.get("text").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        yield Ok(ProviderEvent::ReasoningDelta(text.to_string()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let function = call.get("function").cloned().unwrap_or(Value::Null);
                        let name = function.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let id = call
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("call_{index}"));
                        let partial = partial_tools.entry(index).or_insert_with(|| PartialTool {
                            id: id.clone(),
                            name: String::new(),
                            arguments: String::new(),
                        });
                        if !id.is_empty() {
                            partial.id = id;
                        }
                        if !name.is_empty() {
                            partial.name = name;
                        }
                        if !partial.name.is_empty() && started.insert(index) {
                            yield Ok(ProviderEvent::ToolInputStarted {
                                id: partial.id.clone(),
                                name: partial.name.clone(),
                            });
                        }
                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                            if !arguments.is_empty() {
                                partial.arguments.push_str(arguments);
                                yield Ok(ProviderEvent::ToolInputDelta {
                                    id: partial.id.clone(),
                                    delta: arguments.to_string(),
                                });
                            }
                        }
                    }
                }

                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    if reason == "tool_calls" {
                        for event in finish_tools(&mut partial_tools, &mut ended) {
                            yield Ok(event);
                        }
                    }
                    if !finished {
                        finished = true;
                        yield Ok(ProviderEvent::Finish {
                            reason: map_finish(reason),
                        });
                    }
                }
            }
        }
    };

    Ok(Box::pin(stream))
}

struct PartialTool {
    id: String,
    name: String,
    arguments: String,
}

fn finish_tools(
    partials: &mut BTreeMap<u64, PartialTool>,
    ended: &mut BTreeSet<u64>,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for (index, partial) in partials.iter() {
        if ended.contains(index) || partial.name.is_empty() {
            continue;
        }
        ended.insert(*index);
        let input = serde_json::from_str::<Value>(&partial.arguments)
            .unwrap_or_else(|_| json!({ "_raw": partial.arguments }));
        events.push(ProviderEvent::ToolInputEnded {
            id: partial.id.clone(),
            name: partial.name.clone(),
            input,
        });
    }
    events
}

fn map_finish(reason: &str) -> String {
    match reason {
        "stop" => "stop",
        "length" => "length",
        "tool_calls" | "function_call" => "tool-calls",
        "content_filter" => "content-filter",
        _ => "unknown",
    }
    .to_string()
}

fn to_openai_messages(request: &ProviderRequest) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": request.system})];
    for message in &request.messages {
        match message.role.as_str() {
            "assistant" => {
                let text = message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        oxidecode_core::session::ChatPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let tool_calls: Vec<Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {"name": call.name, "arguments": call.arguments}
                        })
                    })
                    .collect();
                let mut value = json!({"role": "assistant"});
                if !text.is_empty() {
                    value["content"] = json!(text);
                } else {
                    value["content"] = Value::Null;
                }
                if !tool_calls.is_empty() {
                    value["tool_calls"] = Value::Array(tool_calls);
                }
                out.push(value);
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
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": text,
                }));
            }
            _ => {
                let parts: Vec<Value> = message
                    .content
                    .iter()
                    .map(|part| match part {
                        oxidecode_core::session::ChatPart::Text { text } => {
                            json!({"type": "text", "text": text})
                        }
                        oxidecode_core::session::ChatPart::Image { mime, data } => json!({
                            "type": "image_url",
                            "image_url": {"url": format!("data:{mime};base64,{data}")}
                        }),
                    })
                    .collect();
                let has_image = message
                    .content
                    .iter()
                    .any(|part| matches!(part, oxidecode_core::session::ChatPart::Image { .. }));
                if has_image {
                    out.push(json!({"role": "user", "content": parts}));
                } else {
                    let text = message
                        .content
                        .iter()
                        .filter_map(|part| match part {
                            oxidecode_core::session::ChatPart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push(json!({"role": "user", "content": text}));
                }
            }
        }
    }
    out
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}
