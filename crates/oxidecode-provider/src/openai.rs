use crate::catalog::ResolvedModel;
use crate::client::{base_headers, format_http_error, SseDecoder};
use crate::types::{ProviderEvent, ProviderRequest, ProviderStream};
use anyhow::{bail, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn opencode_headers(
    mut builder: reqwest::RequestBuilder,
    model: &crate::catalog::ResolvedModel,
) -> reqwest::RequestBuilder {
    let is_opencode = matches!(
        model.provider_id.as_str(),
        "opencode" | "opencode-zen" | "opencode-go"
    ) || model.base_url.contains("opencode.ai");
    if !is_opencode {
        return builder;
    }

    // Mimic real OpenCode CLI request identity.
    builder = builder
        .header("Accept", "*/*")
        .header("User-Agent", "opencode/stable/2.2.6/cli")
        .header("x-opencode-client", "cli");

    // Real OpenCode sends all session correlation headers; the free gateway
    // rejects requests missing the full session header set.
    if let Some(session_id) = model
        .headers
        .get("x-opencode-session")
        .or_else(|| model.headers.get("x-session-id"))
        .cloned()
    {
        builder = builder
            .header("x-opencode-session", &session_id)
            .header("x-opencode-session-id", &session_id)
            .header("x-session-affinity", &session_id)
            .header("x-session-id", &session_id);
    }

    builder
}

pub(crate) fn build_body(model: &ResolvedModel, request: &ProviderRequest) -> Value {
    let mut body = json!({
        "model": request.model_id,
        "messages": to_openai_messages(request),
        "stream": true,
    });

    let is_opencode = matches!(
        model.provider_id.as_str(),
        "opencode" | "opencode-zen" | "opencode-go"
    ) || model.base_url.contains("opencode.ai");

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    let mut function = json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    });
                    if is_opencode {
                        // Real OpenCode always sets strict:false for function tools.
                        function["strict"] = json!(false);
                    }
                    json!({
                        "type": "function",
                        "function": function,
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
        if is_opencode {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    let supports_usage = is_opencode
        || [
            "openai",
            "openrouter",
            "groq",
            "deepseek",
            "xai",
        ]
        .contains(&model.provider_id.as_str());
    if supports_usage {
        body["stream_options"] = json!({"include_usage": true});
    }
    if model.provider_id == "openrouter" {
        body["provider"] = json!({"sort": "throughput"});
    }
    body
}

pub fn resolve_endpoint_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else if base.ends_with("opencode.ai/inference") || base.ends_with("opencode.ai/inference/go")
    {
        format!("{base}/openai/v1/chat/completions")
    } else if base.ends_with("generativelanguage.googleapis.com/v1beta") {
        format!("{base}/openai/chat/completions")
    } else {
        format!("{base}/chat/completions")
    }
}

pub async fn stream(
    http: &reqwest::Client,
    model: &ResolvedModel,
    request: ProviderRequest,
) -> Result<ProviderStream> {
    let url = resolve_endpoint_url(&model.base_url);
    let body = build_body(model, &request);
    let mut builder = http.post(&url).json(&body);
    if let Some(key) = &model.api_key {
        if !key.is_empty() {
            builder = builder.bearer_auth(key);
        }
    }
    if model.provider_id == "openrouter" {
        builder = builder
            .header("HTTP-Referer", "https://github.com/its-zain-dev/oxidecode")
            .header("X-Title", "OxideCode");
    }
    let builder = base_headers(builder, model);
    let builder = opencode_headers(builder, model);
    let response = builder.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!(
            "{}",
            format_http_error("provider", status.as_u16(), &body, model.is_free)
        );
    }

    let reasoning_fields = reasoning_field_names(model.reasoning_field.as_deref());
    let byte_stream = response.bytes_stream().map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
    Ok(process_sse_stream(byte_stream, reasoning_fields))
}

pub(crate) fn process_sse_stream<S, E>(
    byte_stream: S,
    reasoning_fields: Vec<String>,
) -> ProviderStream
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    let mut decoder = SseDecoder::default();
    let mut partial_tools: BTreeMap<u64, PartialTool> = BTreeMap::new();
    let mut started: BTreeSet<u64> = BTreeSet::new();
    let mut ended: BTreeSet<u64> = BTreeSet::new();
    let mut finished = false;

    let stream = async_stream::stream! {
        tokio::pin!(byte_stream);
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
                    for event in finish_tools(&mut partial_tools, &mut started, &mut ended) {
                        yield Ok(event);
                    }
                    if !finished {
                        let reason = if !ended.is_empty() { "tool-calls" } else { "stop" };
                        yield Ok(ProviderEvent::Finish { reason: reason.into() });
                    }
                    return;
                }
                for event in parse_sse_payload(
                    &payload,
                    &reasoning_fields,
                    &mut partial_tools,
                    &mut started,
                    &mut ended,
                    &mut finished,
                ) {
                    yield Ok(event);
                }
            }
        }

        for payload in decoder.finish() {
            if payload.trim() == "[DONE]" {
                continue;
            }
            for event in parse_sse_payload(
                &payload,
                &reasoning_fields,
                &mut partial_tools,
                &mut started,
                &mut ended,
                &mut finished,
            ) {
                yield Ok(event);
            }
        }

        for event in finish_tools(&mut partial_tools, &mut started, &mut ended) {
            yield Ok(event);
        }
        if !finished {
            let reason = if !ended.is_empty() { "tool-calls" } else { "stop" };
            yield Ok(ProviderEvent::Finish { reason: reason.into() });
        }
    };

    Box::pin(stream)
}

pub(crate) fn reasoning_field_names(custom: Option<&str>) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(custom) = custom.filter(|s| !s.is_empty()) {
        fields.push(custom.to_string());
    }
    for default in ["reasoning_content", "reasoning", "reasoning_text"] {
        if !fields.iter().any(|existing| existing == default) {
            fields.push(default.to_string());
        }
    }
    fields
}

pub(crate) fn parse_sse_payload(
    payload: &str,
    reasoning_fields: &[String],
    partial_tools: &mut BTreeMap<u64, PartialTool>,
    started: &mut BTreeSet<u64>,
    ended: &mut BTreeSet<u64>,
    finished: &mut bool,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    let value: Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(_) => return events,
    };

    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("provider stream error")
            .to_string();
        let status = error
            .get("status")
            .or_else(|| error.get("code"))
            .and_then(Value::as_u64)
            .and_then(|c| u16::try_from(c).ok());
        events.push(ProviderEvent::Error { message, status });
        return events;
    }

    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        let input = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let reasoning = usage
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .or_else(|| usage.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read = usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .or_else(|| usage.get("prompt_cache_hit_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_write = usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cache_write_tokens"))
            .or_else(|| usage.get("prompt_cache_miss_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input > 0 || output > 0 {
            events.push(ProviderEvent::Usage {
                input,
                output,
                reasoning,
                cache_read,
                cache_write,
            });
        }
    }

    let Some(choice) = value.get("choices").and_then(|choices| choices.get(0)) else {
        return events;
    };
    let delta = choice
        .get("delta")
        .or_else(|| choice.get("message"))
        .unwrap_or(&Value::Null);

    if let Some(content) = delta.get("content") {
        match content {
            Value::String(text) if !text.is_empty() => {
                events.push(ProviderEvent::TextDelta(text.clone()));
            }
            Value::Array(parts) => {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            events.push(ProviderEvent::TextDelta(text.to_string()));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for field in reasoning_fields {
        if let Some(reasoning) = delta.get(field.as_str()) {
            match reasoning {
                Value::String(text) if !text.is_empty() => {
                    events.push(ProviderEvent::ReasoningDelta(text.clone()));
                    break;
                }
                Value::Object(map) => {
                    if let Some(text) = map.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            events.push(ProviderEvent::ReasoningDelta(text.to_string()));
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for (pos, call) in calls.iter().enumerate() {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(pos as u64);
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let incoming_name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let incoming_id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);

            let partial = partial_tools.entry(index).or_insert_with(|| PartialTool {
                id: incoming_id
                    .clone()
                    .unwrap_or_else(|| format!("call_{index}")),
                name: String::new(),
                arguments: String::new(),
                emitted_len: 0,
            });

            // Only update `partial.id` when the chunk explicitly provides a non-empty `id`.
            // Subsequent argument delta chunks omit `id` and must preserve the initial ID.
            if let Some(id) = incoming_id {
                partial.id = id;
            }
            if let Some(name) = incoming_name {
                if partial.name.is_empty() {
                    partial.name = name;
                } else if partial.name != name && !started.contains(&index) {
                    partial.name.push_str(&name);
                }
            }

            if let Some(arg_val) = function.get("arguments") {
                match arg_val {
                    Value::String(s) if !s.is_empty() => {
                        partial.arguments.push_str(s);
                    }
                    Value::Object(_) | Value::Array(_) => {
                        let serialized = arg_val.to_string();
                        partial.arguments.push_str(&serialized);
                    }
                    _ => {}
                }
            }

            if !partial.name.is_empty() && started.insert(index) {
                events.push(ProviderEvent::ToolInputStarted {
                    id: partial.id.clone(),
                    name: partial.name.clone(),
                });
            }

            if started.contains(&index) && partial.arguments.len() > partial.emitted_len {
                let delta_str = partial.arguments[partial.emitted_len..].to_string();
                partial.emitted_len = partial.arguments.len();
                events.push(ProviderEvent::ToolInputDelta {
                    id: partial.id.clone(),
                    delta: delta_str,
                });
            }
        }
    }

    if let Some(reason) = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        if !partial_tools.is_empty() {
            events.extend(finish_tools(partial_tools, started, ended));
        }
        if !*finished {
            *finished = true;
            let mapped = if !ended.is_empty() {
                "tool-calls".to_string()
            } else {
                map_finish(reason)
            };
            events.push(ProviderEvent::Finish { reason: mapped });
        }
    }

    events
}

pub(crate) struct PartialTool {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
    pub(crate) emitted_len: usize,
}

pub(crate) fn finish_tools(
    partials: &mut BTreeMap<u64, PartialTool>,
    started: &mut BTreeSet<u64>,
    ended: &mut BTreeSet<u64>,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for (index, partial) in partials.iter_mut() {
        if ended.contains(index) || partial.name.is_empty() {
            continue;
        }
        if started.insert(*index) {
            events.push(ProviderEvent::ToolInputStarted {
                id: partial.id.clone(),
                name: partial.name.clone(),
            });
        }
        if partial.arguments.len() > partial.emitted_len {
            let delta_str = partial.arguments[partial.emitted_len..].to_string();
            partial.emitted_len = partial.arguments.len();
            events.push(ProviderEvent::ToolInputDelta {
                id: partial.id.clone(),
                delta: delta_str,
            });
        }
        ended.insert(*index);
        let trimmed = partial.arguments.trim();
        let input = if trimmed.is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(trimmed)
                .unwrap_or_else(|_| json!({ "_raw": partial.arguments }))
        };
        events.push(ProviderEvent::ToolInputEnded {
            id: partial.id.clone(),
            name: partial.name.clone(),
            input,
        });
    }
    events
}

pub(crate) fn map_finish(reason: &str) -> String {
    match reason {
        "stop" | "end_turn" => "stop",
        "length" | "max_tokens" => "length",
        "tool_calls" | "function_call" | "tool_use" => "tool-calls",
        "content_filter" => "content-filter",
        _ => "unknown",
    }
    .to_string()
}

pub(crate) fn to_openai_messages(request: &ProviderRequest) -> Vec<Value> {
    let mut out = Vec::new();
    if !request.system.trim().is_empty() {
        out.push(json!({"role": "system", "content": request.system}));
    }
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
                        let args = if call.arguments.trim().is_empty() {
                            "{}".to_string()
                        } else {
                            call.arguments.clone()
                        };
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {"name": call.name, "arguments": args}
                        })
                    })
                    .collect();
                let mut value = json!({
                    "role": "assistant",
                    "content": text,
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_tool_call_id_across_multi_chunk_deltas() {
        let mut partial_tools = BTreeMap::new();
        let mut started = BTreeSet::new();
        let mut ended = BTreeSet::new();
        let mut finished = false;
        let fields = reasoning_field_names(Some("reasoning_content"));

        let chunk1 = r#"{"choices":[{"index":0,"delta":{"reasoning_content":"Thinking...","tool_calls":[{"index":0,"id":"call_real_123","type":"function","function":{"name":"read","arguments":""}}]}}]}"#;
        let ev1 = parse_sse_payload(
            chunk1,
            &fields,
            &mut partial_tools,
            &mut started,
            &mut ended,
            &mut finished,
        );
        assert!(matches!(&ev1[0], ProviderEvent::ReasoningDelta(t) if t == "Thinking..."));
        assert!(
            matches!(&ev1[1], ProviderEvent::ToolInputStarted { id, name } if id == "call_real_123" && name == "read")
        );

        // Chunk 2 omits `id` and `name`, streaming only `arguments`
        let chunk2 = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"filePath\":"}}]}}]}"#;
        let ev2 = parse_sse_payload(
            chunk2,
            &fields,
            &mut partial_tools,
            &mut started,
            &mut ended,
            &mut finished,
        );
        assert!(
            matches!(&ev2[0], ProviderEvent::ToolInputDelta { id, delta } if id == "call_real_123" && delta == "{\"filePath\":")
        );

        // Chunk 3 finishes arguments and sends finish_reason
        let chunk3 = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let ev3 = parse_sse_payload(
            chunk3,
            &fields,
            &mut partial_tools,
            &mut started,
            &mut ended,
            &mut finished,
        );
        assert!(
            matches!(&ev3[0], ProviderEvent::ToolInputDelta { id, delta } if id == "call_real_123" && delta == "\"Cargo.toml\"}")
        );
        assert!(
            matches!(&ev3[1], ProviderEvent::ToolInputEnded { id, name, input } if id == "call_real_123" && name == "read" && input["filePath"] == "Cargo.toml")
        );
        assert!(matches!(&ev3[2], ProviderEvent::Finish { reason } if reason == "tool-calls"));
    }
}
