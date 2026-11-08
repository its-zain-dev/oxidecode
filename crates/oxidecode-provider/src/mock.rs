use crate::types::{ProviderEvent, ProviderRequest, ProviderStream};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::sleep;

/// Deterministic offline provider used by tests and by `OXIDECODE_MOCK=1`.
///
/// It echoes the last user turn, emits a short reasoning trace, and exercises
/// the tool pipeline when the prompt starts with a recognisable command:
///   * `:tool <name> <json>` emits exactly one tool call with the given input;
///   * `:slow <text>` streams slowly (for UI/interrupt testing);
///   * anything else returns an echo answer.
pub fn stream(request: ProviderRequest) -> ProviderStream {
    let stream = async_stream::stream! {
        let prompt = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        oxidecode_core::session::ChatPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        yield Ok(ProviderEvent::ReasoningDelta(
            "The mock provider is answering deterministically. ".to_string(),
        ));
        sleep(Duration::from_millis(15)).await;

        if let Some(rest) = prompt.trim().strip_prefix(":tool ") {
            let mut parts = rest.splitn(2, ' ');
            let name = parts.next().unwrap_or("").trim().to_string();
            let input = parts.next().unwrap_or("{}").trim();
            let parsed = serde_json::from_str::<Value>(input).unwrap_or_else(|_| json!({}));
            let id = format!("call_{}", short_hash(&prompt));
            yield Ok(ProviderEvent::ToolInputStarted {
                id: id.clone(),
                name: name.clone(),
            });
            let arguments = parsed.to_string();
            for chunk in split_chunks(&arguments, 24) {
                yield Ok(ProviderEvent::ToolInputDelta {
                    id: id.clone(),
                    delta: chunk,
                });
                sleep(Duration::from_millis(5)).await;
            }
            yield Ok(ProviderEvent::ToolInputEnded {
                id,
                name,
                input: parsed,
            });
            yield Ok(ProviderEvent::Finish { reason: "tool-calls".into() });
            return;
        }

        let slow = prompt.trim().strip_prefix(":slow ").unwrap_or(&prompt).to_string();
        let answer = if prompt.trim().is_empty() {
            "OxideCode mock provider ready. Try `:tool read {\"filePath\":\"Cargo.toml\"}` or ask anything.".to_string()
        } else if prompt.trim().starts_with(':') && !prompt.trim().starts_with(":slow ") {
            format!("Unknown mock command `{}`.", prompt.trim())
        } else {
            format!("Echo: {slow}")
        };

        let step = if prompt.trim().starts_with(":slow ") { 12 } else { 48 };
        for chunk in split_chunks(&answer, step) {
            yield Ok(ProviderEvent::TextDelta(chunk));
            sleep(Duration::from_millis(if step <= 12 { 60 } else { 8 })).await;
        }

        let input_tokens = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|part| match part {
                oxidecode_core::session::ChatPart::Text { text } => Some(text.len()),
                _ => None,
            })
            .sum::<usize>() as u64 / 4;
        yield Ok(ProviderEvent::Usage {
            input: input_tokens,
            output: answer.len() as u64 / 4,
            reasoning: 8,
            cache_read: 0,
            cache_write: 0,
        });
        yield Ok(ProviderEvent::Finish { reason: "stop".into() });
    };

    Box::pin(stream)
}

fn split_chunks(text: &str, size: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= size {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}
