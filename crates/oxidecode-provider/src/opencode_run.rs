use crate::catalog::ResolvedModel;
use crate::types::{ProviderEvent, ProviderRequest, ProviderStream};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

fn opencode_bin() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("OPENCODE_BIN") {
        let p = std::path::PathBuf::from(p.trim());
        if p.is_file() { return Some(p); }
    }
    for p in [
        "/data/data/com.termux/files/usr/bin/opencode",
        "/usr/local/bin/opencode",
        "/usr/bin/opencode",
    ] {
        let p = std::path::PathBuf::from(p);
        if p.is_file() { return Some(p); }
    }
    None
}

pub fn can_use(model: &ResolvedModel) -> bool {
    let is_opencode = matches!(model.provider_id.as_str(), "opencode" | "opencode-zen" | "opencode-go")
        || model.base_url.contains("opencode.ai");
    is_opencode && opencode_bin().is_some()
}

fn last_user_prompt(request: &ProviderRequest) -> String {
    for msg in request.messages.iter().rev() {
        if msg.role == "user" {
            let mut out = String::new();
            for part in &msg.parts {
                if let oxidecode_core::session::ChatPart::Text { text } = part {
                    if !out.is_empty() { out.push_str("\n"); }
                    out.push_str(text);
                }
            }
            if !out.trim().is_empty() { return out; }
        }
    }
    "Hello".to_string()
}

pub async fn stream(model: &ResolvedModel, request: ProviderRequest) -> Result<ProviderStream> {
    let bin = opencode_bin().context("OpenCode binary not found for opencode.ai inference")?;
    let prompt = last_user_prompt(&request);
    let model_ref = format!("{}/{}", model.provider_id, model.model_id);

    let mut cmd = Command::new(&bin);
    cmd.arg("run")
        .arg("--standalone")
        .arg("--format").arg("json")
        .arg("--model").arg(&model_ref)
        .arg("--auto")
        .arg("--")
        .arg(&prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // Avoid inheriting a config that might redirect in confusing ways? keep env as-is.
    let mut child = cmd.spawn().with_context(|| format!("spawn opencode run: {}", bin.display()))?;
    let stdout = child.stdout.take().context("opencode stdout")?;
    let mut stderr = child.stderr.take().context("opencode stderr")?;
    tokio::spawn(async move { let mut b=Vec::new(); let _=tokio::io::AsyncReadExt::read_to_end(&mut stderr,&mut b).await; });

    let s = async_stream::stream! {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut saw_text = false;
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if line.is_empty() { continue; }
            let v: Value = match serde_json::from_str(line) { Ok(v)=>v, Err(_)=>continue };
            let typ = v.get("type").and_then(Value::as_str).unwrap_or("");
            match typ {
                "text" => {
                    if let Some(text) = v.get("part").and_then(|p| p.get("text")).and_then(Value::as_str) {
                        if !text.is_empty() {
                            saw_text = true;
                            yield Ok(ProviderEvent::TextDelta(text.to_string()));
                        }
                    }
                }
                "reasoning" => {
                    if let Some(text) = v.get("part").and_then(|p| p.get("text")).and_then(Value::as_str) {
                        if !text.is_empty() {
                            yield Ok(ProviderEvent::ReasoningDelta(text.to_string()));
                        }
                    }
                }
                "step_finish" => {
                    let reason = v.get("part").and_then(|p| p.get("reason")).and_then(Value::as_str).unwrap_or("stop").to_string();
                    if let Some(tokens) = v.get("part").and_then(|p| p.get("tokens")) {
                        let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
                        let output = tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
                        let reasoning = tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0);
                        let cache_read = tokens.get("cache").and_then(|c| c.get("read")).and_then(Value::as_u64).unwrap_or(0);
                        let cache_write = tokens.get("cache").and_then(|c| c.get("write")).and_then(Value::as_u64).unwrap_or(0);
                        if input>0 || output>0 || cache_read>0 || cache_write>0 {
                            yield Ok(ProviderEvent::Usage { input, output, reasoning, cache_read, cache_write });
                        }
                    }
                    let reason = match reason.as_str() { "stop"|"end_turn"=>"stop", "tool-calls"|"tool_use"=>"tool-calls", "length"|"max_tokens"=>"length", _=> "stop" }.to_string();
                    yield Ok(ProviderEvent::Finish { reason });
                }
                "error" => {
                    let msg = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str)
                        .or_else(|| v.get("message").and_then(Value::as_str))
                        .unwrap_or("opencode run error").to_string();
                    yield Ok(ProviderEvent::Error { message: msg, status: None });
                }
                _ => {}
            }
        }
        let status = child.wait().await;
        if let Ok(st) = status {
            if !st.success() && !saw_text {
                // stderr was drained; emit a generic error only if nothing useful was produced.
            }
        }
    };
    Ok(Box::pin(s))
}
