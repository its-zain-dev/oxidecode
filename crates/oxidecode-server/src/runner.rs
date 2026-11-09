use crate::prompt::system_prompt;
use crate::state::AppState;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use oxidecode_core::config::LoadedConfig;
use oxidecode_core::ids;
use oxidecode_core::session::{
    AssistantMessage, AssistantPart, ChatMessage, ChatPart, ChatToolCall, Message, ModelRef,
    SessionInfo, StructuredError, TokenUsage, ToolPart, ToolState,
};
use oxidecode_provider::catalog::{self, ResolvedModel};
use oxidecode_provider::{ProviderEvent, ProviderRequest};
use oxidecode_tools::{ToolContext, ToolError, ToolOutput};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed,
    Interrupted,
}

/// Spawns the agent loop for a session. The prompt endpoint has already stored
/// the user message.
pub fn spawn(state: AppState, session_id: String) {
    tokio::spawn(async move {
        if let Err(error) = run(state.clone(), &session_id).await {
            tracing::warn!(session = %session_id, error = %error, "agent run failed");
            let directory = state
                .store()
                .get_session(&session_id)
                .ok()
                .flatten()
                .map(|session| session.location.directory)
                .unwrap_or_else(|| state.default_directory().display().to_string());
            state.bus().emit_session(
                "session.execution.failed",
                &directory,
                json!({
                    "sessionID": session_id,
                    "error": {
                        "type": "UnknownError",
                        "message": error.to_string(),
                    }
                }),
            );
            state.finish_runtime(&session_id);
        }
    });
}

pub async fn run(state: AppState, session_id: &str) -> Result<()> {
    let store = state.store().clone();
    let mut session = store
        .get_session(session_id)?
        .with_context(|| format!("session {session_id} not found"))?;
    let directory = PathBuf::from(&session.location.directory);
    let config = state.config_for(&directory);

    let session_had_stale_mock = session
        .model
        .as_ref()
        .map(|m| m.provider_id == "mock" && !catalog::is_mock_enabled())
        .unwrap_or(false);

    let model_ref = session
        .model
        .clone()
        .filter(|m| m.provider_id != "mock" || catalog::is_mock_enabled())
        .or_else(|| {
            config
                .config
                .model
                .as_deref()
                .and_then(|value| config.parse_model(value))
                .map(|(provider, id, variant)| ModelRef {
                    provider_id: provider,
                    id,
                    variant,
                })
        })
        .or_else(|| {
            catalog::default_model(&config).map(|model| ModelRef {
                provider_id: model.provider_id,
                id: model.model_id,
                variant: None,
            })
        });

    let Some(model_ref) = model_ref else {
        fail_without_model(&state, &session, &config).await;
        return Ok(());
    };
    if session.model.is_none() || session_had_stale_mock {
        session.model = Some(model_ref.clone());
        store.update_session(&session)?;
    }

    let Some(mut resolved) = catalog::resolve(&config, &model_ref) else {
        let error = StructuredError::new(
            "ModelNotFoundError",
            format!(
                "model {}/{} is not available",
                model_ref.provider_id, model_ref.id
            ),
        );
        finish_failure(&state, &mut session, error.clone()).await;
        return Ok(());
    };

    if (resolved.provider_id == "opencode"
        || resolved.provider_id == "opencode-zen"
        || resolved.provider_id == "opencode-go"
        || resolved.base_url.contains("opencode.ai"))
        && !resolved.headers.contains_key("x-opencode-session")
    {
        resolved
            .headers
            .insert("x-opencode-session".to_string(), session_id.to_string());
    }

    let abort = state.register_runtime(session_id, "");
    let policy = crate::permission::policy_for(&state, &session, &config, abort.clone());

    mark_busy(&state, &mut session);
    state.bus().emit_session(
        "session.execution.started",
        &session.location.directory,
        json!({ "sessionID": session_id }),
    );

    let mut outcome = Outcome::Succeeded;
    let mut last_error: Option<StructuredError> = None;
    let mut history_messages_raw = store.all_messages(session_id)?;
    let mut step = 0usize;

    loop {
        step += 1;
        if step > 100 {
            outcome = Outcome::Failed;
            last_error = Some(StructuredError::new(
                "StepLimitError",
                "the agent exceeded the maximum number of steps (100)",
            ));
            break;
        }
        if *abort.borrow() {
            outcome = Outcome::Interrupted;
            break;
        }

        let history = build_history(&history_messages_raw);
        let mut assistant = AssistantMessage::new(
            session.agent.clone().unwrap_or_else(|| "build".to_string()),
            model_ref.clone(),
        );
        let message = Message::Assistant(assistant.clone());
        store.append_message(session_id, &message)?;
        history_messages_raw.push(message.clone());

        let agent = assistant.agent.clone();
        let system = system_prompt(&config, &directory, &agent, &resolved.display());
        let tools = tool_specs(&config, &agent);
        let request = ProviderRequest {
            provider_id: resolved.provider_id.clone(),
            model_id: resolved.model_id.clone(),
            variant: resolved.variant.clone(),
            system,
            messages: history,
            tools,
            temperature: None,
            max_tokens: None,
        };

        state.bus().emit_session(
            "session.step.started",
            &session.location.directory,
            json!({
                "sessionID": session_id,
                "assistantMessageID": assistant.id,
                "agent": assistant.agent,
                "model": model_ref,
                "started": ids::now_ms(),
            }),
        );

        let stream = match state.client().stream(&resolved, request).await {
            Ok(stream) => stream,
            Err(error) => {
                let structured = provider_error(&error);
                assistant.error = Some(structured.clone());
                assistant.time.completed = Some(ids::now_ms());
                let message = Message::Assistant(assistant.clone());
                let _ = store.update_message(session_id, &message);
                replace_message(&mut history_messages_raw, message);
                state.bus().emit_session(
                    "session.step.failed",
                    &session.location.directory,
                    json!({
                        "sessionID": session_id,
                        "assistantMessageID": assistant.id,
                        "error": structured,
                    }),
                );
                last_error = Some(structured);
                outcome = Outcome::Failed;
                break;
            }
        };
        tokio::pin!(stream);

        let mut text_ordinal = 0usize;
        let mut reasoning_ordinal = 0usize;
        let mut text_open = false;
        let mut reasoning_open = false;
        let mut finish = "stop".to_string();
        let mut usage = TokenUsage::default();
        let mut provider_failure: Option<StructuredError> = None;

        loop {
            let mut abort_signal = abort.clone();
            let event = tokio::select! {
                item = stream.next() => item,
                _ = wait_abort(&mut abort_signal) => {
                    outcome = Outcome::Interrupted;
                    None
                }
            };
            let Some(event) = event else {
                if outcome == Outcome::Interrupted {
                    break;
                }
                break;
            };
            match event {
                Ok(ProviderEvent::TextDelta(delta)) => {
                    if !text_open {
                        assistant.push_text("");
                        text_open = true;
                        state.bus().emit_session(
                            "session.text.started",
                            &session.location.directory,
                            json!({
                                "sessionID": session_id,
                                "assistantMessageID": assistant.id,
                                "ordinal": text_ordinal,
                            }),
                        );
                    }
                    assistant.push_text(&delta);
                    state.bus().emit_session(
                        "session.text.delta",
                        &session.location.directory,
                        json!({
                            "sessionID": session_id,
                            "assistantMessageID": assistant.id,
                            "ordinal": text_ordinal,
                            "delta": delta,
                        }),
                    );
                }
                Ok(ProviderEvent::ReasoningDelta(delta)) => {
                    if !reasoning_open {
                        assistant.push_reasoning("");
                        reasoning_open = true;
                        state.bus().emit_session(
                            "session.reasoning.started",
                            &session.location.directory,
                            json!({
                                "sessionID": session_id,
                                "assistantMessageID": assistant.id,
                                "ordinal": reasoning_ordinal,
                            }),
                        );
                    }
                    assistant.push_reasoning(&delta);
                    state.bus().emit_session(
                        "session.reasoning.delta",
                        &session.location.directory,
                        json!({
                            "sessionID": session_id,
                            "assistantMessageID": assistant.id,
                            "ordinal": reasoning_ordinal,
                            "delta": delta,
                        }),
                    );
                }
                Ok(ProviderEvent::ToolInputStarted { id, name }) => {
                    if text_open {
                        emit_text_ended(&state, &session, &assistant, text_ordinal);
                        text_open = false;
                        text_ordinal += 1;
                    }
                    if reasoning_open {
                        emit_reasoning_ended(&state, &session, &assistant, reasoning_ordinal);
                        reasoning_open = false;
                        reasoning_ordinal += 1;
                    }
                    let part = ToolPart::new(id.clone(), name.clone(), String::new());
                    assistant.content.push(AssistantPart::Tool(part));
                    state.bus().emit_session(
                        "session.tool.input.started",
                        &session.location.directory,
                        json!({
                            "sessionID": session_id,
                            "assistantMessageID": assistant.id,
                            "id": id,
                            "name": name,
                        }),
                    );
                }
                Ok(ProviderEvent::ToolInputDelta { id, delta }) => {
                    if let Some(part) = tool_part_mut(&mut assistant, &id) {
                        if let ToolState::Streaming { input } = &mut part.state {
                            input.push_str(&delta);
                        }
                    }
                    state.bus().emit_session(
                        "session.tool.input.delta",
                        &session.location.directory,
                        json!({
                            "sessionID": session_id,
                            "assistantMessageID": assistant.id,
                            "id": id,
                            "delta": delta,
                        }),
                    );
                }
                Ok(ProviderEvent::ToolInputEnded { id, name, input }) => {
                    if let Some(part) = tool_part_mut(&mut assistant, &id) {
                        part.name = name.clone();
                        part.state = ToolState::Running {
                            input: input.clone(),
                            metadata: json!({}),
                        };
                    }
                    state.bus().emit_session(
                        "session.tool.input.ended",
                        &session.location.directory,
                        json!({
                            "sessionID": session_id,
                            "assistantMessageID": assistant.id,
                            "id": id,
                            "text": input.to_string(),
                        }),
                    );
                }
                Ok(ProviderEvent::Usage {
                    input,
                    output,
                    reasoning,
                    cache_read,
                    cache_write,
                }) => {
                    usage = TokenUsage {
                        input,
                        output,
                        reasoning,
                        cache: oxidecode_core::session::CacheUsage {
                            read: cache_read,
                            write: cache_write,
                        },
                    };
                }
                Ok(ProviderEvent::Finish { reason }) => {
                    finish = reason;
                }
                Ok(ProviderEvent::Error { message, status }) => {
                    let mut error = StructuredError::new("ProviderError", message);
                    error.status = status;
                    provider_failure = Some(error);
                    break;
                }
                Err(error) => {
                    provider_failure = Some(provider_error(&error));
                    break;
                }
            }
        }

        if text_open {
            emit_text_ended(&state, &session, &assistant, text_ordinal);
            let _ = text_ordinal;
        }
        if reasoning_open {
            emit_reasoning_ended(&state, &session, &assistant, reasoning_ordinal);
        }

        assistant.tokens = Some(usage.clone());
        assistant.finish = Some(finish.clone());
        assistant.time.streamed = Some(ids::now_ms());
        assistant.time.completed = Some(ids::now_ms());

        if outcome == Outcome::Interrupted {
            assistant.error = Some(StructuredError::new("InterruptedError", "execution interrupted"));
            let message = Message::Assistant(assistant.clone());
            let _ = store.update_message(session_id, &message);
            break;
        }

        if let Some(error) = provider_failure {
            assistant.error = Some(error.clone());
            let message = Message::Assistant(assistant.clone());
            let _ = store.update_message(session_id, &message);
            replace_message(&mut history_messages_raw, message);
            state.bus().emit_session(
                "session.step.failed",
                &session.location.directory,
                json!({
                    "sessionID": session_id,
                    "assistantMessageID": assistant.id,
                    "error": error,
                }),
            );
            last_error = Some(error);
            outcome = Outcome::Failed;
            break;
        }

        let cost = step_cost(&config, &model_ref, &usage);
        assistant.cost = Some(cost);
        let message = Message::Assistant(assistant.clone());
        store.update_message(session_id, &message)?;
        replace_message(&mut history_messages_raw, message);

        state.bus().emit_session(
            "session.step.ended",
            &session.location.directory,
            json!({
                "sessionID": session_id,
                "assistantMessageID": assistant.id,
                "finish": finish,
                "cost": cost,
                "tokens": usage,
            }),
        );
        session.cost += cost;
        session.tokens.input += usage.input;
        session.tokens.output += usage.output;
        session.tokens.reasoning += usage.reasoning;
        session.tokens.cache.read += usage.cache.read;
        session.tokens.cache.write += usage.cache.write;
        store.update_session(&session)?;

        let pending: Vec<String> = assistant
            .content
            .iter()
            .filter_map(|part| match part {
                AssistantPart::Tool(tool) => match &tool.state {
                    ToolState::Running { .. } => Some(tool.id.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        if pending.is_empty() {
            break;
        }

        for tool_id in pending {
            if *abort.borrow() {
                outcome = Outcome::Interrupted;
                break;
            }
            let Some(index) = assistant.content.iter().position(|part| {
                matches!(part, AssistantPart::Tool(tool) if tool.id == tool_id)
            }) else {
                continue;
            };
            let (name, input) = match &assistant.content[index] {
                AssistantPart::Tool(tool) => {
                    let input = match &tool.state {
                        ToolState::Running { input, .. } => input.clone(),
                        ToolState::Streaming { input } => {
                            serde_json::from_str(input).unwrap_or(json!({}))
                        }
                        _ => json!({}),
                    };
                    (tool.name.clone(), input)
                }
                _ => continue,
            };

            state.bus().emit_session(
                "session.tool.called",
                &session.location.directory,
                json!({
                    "sessionID": session_id,
                    "assistantMessageID": assistant.id,
                    "id": tool_id,
                    "name": name,
                    "input": input,
                    "executed": true,
                }),
            );

            let tool_context = ToolContext {
                directory: directory.clone(),
                session_id: session_id.to_string(),
                message_id: assistant.id.clone(),
                agent: agent.clone(),
                store: store.clone(),
                bus: state.bus().clone(),
                config: config.clone(),
                policy: policy.clone(),
                abort: abort.clone(),
            };

            let result: Result<ToolOutput, ToolError> =
                oxidecode_tools::execute(&name, &tool_context, input.clone()).await;

            match result {
                Ok(output) => {
                    if let Some(part) = tool_part_mut(&mut assistant, &tool_id) {
                        part.executed = true;
                        part.time.ran = Some(ids::now_ms());
                        part.time.completed = Some(ids::now_ms());
                        part.state = ToolState::Completed {
                            input: input.clone(),
                            content: vec![json!({"type": "text", "text": output.output})],
                            metadata: output.metadata.clone(),
                        };
                        state.bus().emit_session(
                            "session.tool.success",
                            &session.location.directory,
                            json!({
                                "sessionID": session_id,
                                "assistantMessageID": assistant.id,
                                "id": tool_id,
                                "name": name,
                                "content": [{"type": "text", "text": output.output}],
                                "metadata": output.metadata,
                                "executed": true,
                            }),
                        );
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let structured = StructuredError::new(
                        match error {
                            ToolError::Denied(_) => "PermissionDeniedError",
                            ToolError::Invalid(_) => "InvalidToolInputError",
                            ToolError::Failed(_) => "ToolExecutionError",
                        },
                        message.clone(),
                    );
                    if let Some(part) = tool_part_mut(&mut assistant, &tool_id) {
                        part.executed = true;
                        part.time.ran = Some(ids::now_ms());
                        part.time.completed = Some(ids::now_ms());
                        part.state = ToolState::Error {
                            input: input.clone(),
                            error: structured.clone(),
                            content: vec![json!({"type": "text", "text": message})],
                            metadata: json!({}),
                        };
                        state.bus().emit_session(
                            "session.tool.failed",
                            &session.location.directory,
                            json!({
                                "sessionID": session_id,
                                "assistantMessageID": assistant.id,
                                "id": tool_id,
                                "name": name,
                                "error": structured,
                                "content": [{"type": "text", "text": message}],
                                "metadata": {},
                                "executed": true,
                            }),
                        );
                    }
                }
            }

            let message = Message::Assistant(assistant.clone());
            store.update_message(session_id, &message)?;
            replace_message(&mut history_messages_raw, message);
        }

        if outcome == Outcome::Interrupted {
            break;
        }
    }

    state.finish_runtime(session_id);
    match outcome {
        Outcome::Succeeded => {
            append_idle(&store, session_id, "succeeded", None);
            set_outcome(&store, &mut session, "succeeded");
            state.bus().emit_session(
                "session.execution.succeeded",
                &session.location.directory,
                json!({ "sessionID": session_id }),
            );
        }
        Outcome::Failed => {
            let error = last_error
                .unwrap_or_else(|| StructuredError::new("UnknownError", "execution failed"));
            append_idle(&store, session_id, "failed", Some(error.clone()));
            set_outcome(&store, &mut session, "failed");
            state.bus().emit_session(
                "session.execution.failed",
                &session.location.directory,
                json!({ "sessionID": session_id, "error": error }),
            );
        }
        Outcome::Interrupted => {
            append_idle(&store, session_id, "interrupted", None);
            set_outcome(&store, &mut session, "interrupted");
            state.bus().emit_session(
                "session.execution.interrupted",
                &session.location.directory,
                json!({ "sessionID": session_id, "reason": "user" }),
            );
        }
    }
    state.bus().emit_session(
        "session.idle",
        &session.location.directory,
        json!({ "sessionID": session_id }),
    );
    Ok(())
}

async fn wait_abort(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn tool_part_mut<'a>(
    assistant: &'a mut AssistantMessage,
    id: &str,
) -> Option<&'a mut ToolPart> {
    assistant.content.iter_mut().find_map(|part| match part {
        AssistantPart::Tool(tool) if tool.id == id => Some(tool),
        _ => None,
    })
}

fn emit_text_ended(
    state: &AppState,
    session: &SessionInfo,
    assistant: &AssistantMessage,
    ordinal: usize,
) {
    let text = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    state.bus().emit_session(
        "session.text.ended",
        &session.location.directory,
        json!({
            "sessionID": session.id,
            "assistantMessageID": assistant.id,
            "ordinal": ordinal,
            "text": text,
        }),
    );
}

fn emit_reasoning_ended(
    state: &AppState,
    session: &SessionInfo,
    assistant: &AssistantMessage,
    ordinal: usize,
) {
    let text = assistant
        .content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::Reasoning { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    state.bus().emit_session(
        "session.reasoning.ended",
        &session.location.directory,
        json!({
            "sessionID": session.id,
            "assistantMessageID": assistant.id,
            "ordinal": ordinal,
            "text": text,
        }),
    );
}

fn build_history(messages: &[Message]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => {
                let mut content = vec![ChatPart::Text {
                    text: user.text.clone(),
                }];
                if let Some(files) = files_to_parts(&user.files) {
                    content.extend(files);
                }
                out.push(ChatMessage {
                    role: "user".to_string(),
                    content,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
            Message::Assistant(assistant) => {
                let mut content: Vec<ChatPart> = Vec::new();
                let mut tool_calls: Vec<ChatToolCall> = Vec::new();
                for part in &assistant.content {
                    match part {
                        AssistantPart::Text { text, .. } => {
                            if !text.is_empty() {
                                content.push(ChatPart::Text { text: text.clone() });
                            }
                        }
                        AssistantPart::Tool(tool) => {
                            let input = match &tool.state {
                                ToolState::Running { input, .. }
                                | ToolState::Completed { input, .. }
                                | ToolState::Error { input, .. } => input.clone(),
                                ToolState::Streaming { input } => {
                                    serde_json::from_str(input).unwrap_or(json!({}))
                                }
                            };
                            tool_calls.push(ChatToolCall {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                                arguments: input.to_string(),
                            });
                        }
                        AssistantPart::Reasoning { .. } => {}
                    }
                }
                if content.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                out.push(ChatMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls: tool_calls.clone(),
                    tool_call_id: None,
                });
                for call in tool_calls {
                    let output = assistant
                        .content
                        .iter()
                        .find_map(|part| match part {
                            AssistantPart::Tool(tool) if tool.id == call.id => match &tool.state {
                                ToolState::Completed { content, .. } => Some(
                                    content
                                        .iter()
                                        .filter_map(|value| value.get("text").and_then(Value::as_str))
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                ),
                                ToolState::Error { error, .. } => {
                                    Some(format!("Error: {}", error.message))
                                }
                                _ => None,
                            },
                            _ => None,
                        })
                        .unwrap_or_else(|| "Tool did not run.".to_string());
                    out.push(ChatMessage {
                        role: "tool".to_string(),
                        content: vec![ChatPart::Text { text: output }],
                        tool_calls: Vec::new(),
                        tool_call_id: Some(call.id),
                    });
                }
            }
            Message::System(system) => {
                out.push(ChatMessage {
                    role: "user".to_string(),
                    content: vec![ChatPart::Text {
                        text: format!("[system] {}", system.text),
                    }],
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
            Message::Synthetic(synthetic) => {
                out.push(ChatMessage {
                    role: "user".to_string(),
                    content: vec![ChatPart::Text {
                        text: format!("[synthetic] {}", synthetic.text),
                    }],
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
            _ => {}
        }
    }
    out
}

fn files_to_parts(files: &[Value]) -> Option<Vec<ChatPart>> {
    if files.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for file in files {
        let uri = file.get("uri").and_then(Value::as_str).unwrap_or("");
        let name = file
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("attachment");
        if let Some(rest) = uri.strip_prefix("data:") {
            let Some((meta, payload)) = rest.split_once(',') else {
                continue;
            };
            let mime = meta.split(';').next().unwrap_or("application/octet-stream");
            let is_base64 = meta.split(';').any(|part| part == "base64");
            let decoded = if is_base64 {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
            } else {
                Some(payload.to_string())
            };
            if mime.starts_with("image/") {
                out.push(ChatPart::Image {
                    mime: mime.to_string(),
                    data: payload.to_string(),
                });
            } else if let Some(text) = decoded {
                out.push(ChatPart::Text {
                    text: format!("\n\n[Attachment: {name}]\n{text}"),
                });
            }
        } else if uri.starts_with("file:") {
            if let Some(path) = uri.strip_prefix("file://") {
                if let Ok(text) = std::fs::read_to_string(path) {
                    out.push(ChatPart::Text {
                        text: format!("\n\n[Attachment: {name}]\n{text}"),
                    });
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn replace_message(messages: &mut [Message], message: Message) {
    let id = message.id().to_string();
    if let Some(existing) = messages
        .iter_mut()
        .find(|existing| existing.id() == id)
    {
        *existing = message;
    }
}

fn tool_specs(config: &Arc<LoadedConfig>, agent: &str) -> Vec<oxidecode_provider::ToolSpec> {
    let mut definitions = oxidecode_tools::definitions();
    if let Some(agent_config) = config.config.agents.get(agent) {
        if !agent_config.tools.is_empty() {
            definitions.retain(|tool| {
                agent_config
                    .tools
                    .get(&tool.name)
                    .copied()
                    .unwrap_or(true)
            });
        }
    }
    definitions
        .into_iter()
        .map(|tool| oxidecode_provider::ToolSpec {
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        })
        .collect()
}

fn step_cost(config: &LoadedConfig, model: &ModelRef, usage: &TokenUsage) -> f64 {
    let model_info = catalog::models(config)
        .into_iter()
        .find(|info| info.provider_id == model.provider_id && info.model_id == model.id);
    let Some(info) = model_info else {
        return 0.0;
    };
    let Some(cost) = info.cost.get(0) else {
        return 0.0;
    };
    let input = cost.get("input").and_then(Value::as_f64).unwrap_or(0.0);
    let output = cost.get("output").and_then(Value::as_f64).unwrap_or(0.0);
    let read = cost
        .get("cache")
        .and_then(|cache| cache.get("read"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let write = cost
        .get("cache")
        .and_then(|cache| cache.get("write"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    (usage.input as f64 * input
        + usage.output as f64 * output
        + usage.cache.read as f64 * read
        + usage.cache.write as f64 * write)
        / 1_000_000.0
}

fn provider_error(error: &anyhow::Error) -> StructuredError {
    let text = error.to_string();
    let status = text
        .split("HTTP ")
        .nth(1)
        .and_then(|rest| rest.split(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|code| code.parse::<u16>().ok());
    let mut structured = StructuredError::new("ProviderError", text);
    structured.status = status;
    structured
}

fn mark_busy(state: &AppState, session: &mut SessionInfo) {
    let now = ids::now_ms();
    session.outcome = None;
    session.time.busy = Some(now);
    session.time.updated = now;
    let _ = state.store().update_session(session);
}

fn set_outcome(store: &oxidecode_core::store::Store, session: &mut SessionInfo, outcome: &str) {
    let now = ids::now_ms();
    session.outcome = Some(outcome.to_string());
    session.time.idle = Some(now);
    session.time.updated = now;
    let _ = store.update_session(session);
}

fn append_idle(
    store: &oxidecode_core::store::Store,
    session_id: &str,
    outcome: &str,
    error: Option<StructuredError>,
) {
    let mut idle = match Message::idle(outcome) {
        Message::Idle(idle) => idle,
        _ => return,
    };
    idle.error = error;
    let _ = store.append_message(session_id, &Message::Idle(idle));
}

async fn fail_without_model(
    state: &AppState,
    session: &SessionInfo,
    config: &Arc<LoadedConfig>,
) {
    let error = StructuredError::new(
        "NoModelError",
        "no model is configured. Set `model` in opencode.json or export a provider API key.",
    );
    let _ = config;
    state.bus().emit_session(
        "session.execution.failed",
        &session.location.directory,
        json!({ "sessionID": session.id, "error": error }),
    );
    state.bus().emit_session(
        "session.idle",
        &session.location.directory,
        json!({ "sessionID": session.id }),
    );
}

async fn finish_failure(
    state: &AppState,
    session: &mut SessionInfo,
    error: StructuredError,
) {
    append_idle(state.store(), &session.id, "failed", Some(error.clone()));
    set_outcome(state.store(), session, "failed");
    state.bus().emit_session(
        "session.execution.failed",
        &session.location.directory,
        json!({ "sessionID": session.id, "error": error }),
    );
    tokio::time::sleep(Duration::from_millis(0)).await;
}

fn _unused(_: ResolvedModel) {}
