use super::context::{PermissionRequest, ToolContext, ToolError, ToolOutput};
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub command: String,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    if context.aborted() {
        return Err(ToolError::failed("aborted"));
    }
    let workdir = match input.workdir.as_deref() {
        Some(dir) if !dir.trim().is_empty() => context.resolve(dir)?,
        _ => context.directory.clone(),
    };
    let relative_dir = if workdir == context.directory {
        ".".to_string()
    } else {
        context.relative(&workdir)
    };

    context
        .authorize(PermissionRequest {
            action: "bash".into(),
            resources: vec![input.command.clone()],
            save: vec![],
            metadata: Some(json!({
                "command": input.command,
                "workdir": relative_dir,
                "description": input.description,
            })),
        })
        .await?;

    let shell = context
        .config
        .config
        .shell
        .clone()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/system/bin/sh".to_string());
    let timeout_ms = input
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);

    let mut command = tokio::process::Command::new(&shell);
    command
        .arg("-c")
        .arg(&input.command)
        .current_dir(&workdir)
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = command
        .spawn()
        .map_err(|error| ToolError::failed(format!("failed to start {shell}: {error}")))?;
    let pid = child.id();
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(256);

    if let Some(mut stdout) = child.stdout.take() {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 8192];
            loop {
                match stdout.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send(buffer[..read].to_vec()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    if let Some(mut stderr) = child.stderr.take() {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 8192];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send(buffer[..read].to_vec()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(sender);

    let mut captured: Vec<u8> = Vec::new();
    let mut total_bytes: usize = 0;
    let mut timed_out = false;
    let mut aborted = false;
    let started = std::time::Instant::now();
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);

    let status = loop {
        tokio::select! {
            chunk = receiver.recv() => {
                match chunk {
                    Some(bytes) => {
                        total_bytes += bytes.len();
                        if captured.len() < MAX_CAPTURE_BYTES {
                            captured.extend_from_slice(&bytes);
                        }
                        context.bus.emit_session(
                            "session.tool.output",
                            &context.directory.display().to_string(),
                            json!({
                                "sessionID": context.session_id,
                                "messageID": context.message_id,
                                "tool": "bash",
                                "chunk": String::from_utf8_lossy(&bytes),
                            }),
                        );
                    }
                    None => continue,
                }
            }
            result = child.wait() => {
                break result.ok();
            }
            _ = &mut deadline => {
                timed_out = true;
                terminate(pid).await;
                let _ = child.wait().await;
                break None;
            }
            _ = async {
                let mut abort = context.abort.clone();
                while !*abort.borrow() {
                    if abort.changed().await.is_err() {
                        break;
                    }
                }
            } => {
                aborted = true;
                terminate(pid).await;
                let _ = child.wait().await;
                break None;
            }
        }
    };

    // Drain whatever is still buffered in the pipes.
    let drain = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(bytes) = receiver.recv().await {
            total_bytes += bytes.len();
            if captured.len() < MAX_CAPTURE_BYTES {
                captured.extend_from_slice(&bytes);
            }
        }
    })
    .await;
    let _ = drain;

    let text = String::from_utf8_lossy(&captured).into_owned();
    let (text, truncated) = context.truncate(&text);
    let exit_code = status.and_then(|status| status.code());
    let relative = context.relative(&workdir);
    let header = format!(
        "{} in {} ({}ms)",
        input
            .description
            .clone()
            .unwrap_or_else(|| input.command.clone()),
        relative,
        started.elapsed().as_millis()
    );

    let metadata = json!({
        "command": input.command,
        "workdir": relative,
        "exit": exit_code,
        "timedOut": timed_out,
        "truncated": truncated,
        "bytes": total_bytes,
    });

    if timed_out {
        return Err(ToolError::failed(format!(
            "Command timed out after {timeout_ms} ms.\n{text}"
        )));
    }
    if aborted || context.aborted() {
        return Err(ToolError::failed("Command interrupted.".to_string()));
    }
    match exit_code {
        Some(0) => Ok(ToolOutput::new(
            format!("{header}: completed"),
            text,
            metadata,
        )),
        Some(code) => Err(ToolError::failed(format!(
            "Command exited with code {code}.\n{text}"
        ))),
        None => Err(ToolError::failed(format!(
            "Command was terminated.\n{text}"
        ))),
    }
}

async fn terminate(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            // Negative pid targets the process group created by setsid().
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}
