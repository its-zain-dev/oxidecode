use super::context::{ToolContext, ToolError, ToolOutput};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use tokio::io::AsyncReadExt;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub file_path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let path = context.resolve(&input.file_path)?;
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| ToolError::invalid(format!("{}: {error}", path.display())))?;
    if metadata.is_dir() {
        return Err(ToolError::invalid(format!(
            "{} is a directory; use the list tool",
            path.display()
        )));
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(ToolError::failed(format!(
            "file is too large ({} bytes)",
            metadata.len()
        )));
    }

    let mut bytes = Vec::with_capacity(metadata.len().min((MAX_BYTES as u64) * 2) as usize);
    tokio::fs::File::open(&path)
        .await
        .map_err(|error| ToolError::failed(error.to_string()))?
        .take(MAX_ATTACHMENT_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ToolError::failed(error.to_string()))?;

    if let Some(mime) = detect_image(&path, &bytes) {
        return Ok(image_attachment(context, &path, mime, bytes));
    }

    if is_binary(&bytes) {
        return Ok(ToolOutput::new(
            format!("Read {}", context.relative(&path)),
            format!(
                "Binary file: {} ({} bytes). Content not shown.",
                context.relative(&path),
                bytes.len()
            ),
            json!({ "filePath": context.relative(&path), "binary": true, "size": bytes.len() }),
        ));
    }

    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let offset = input.offset.unwrap_or(0);
    if offset > 0 && offset >= total {
        return Err(ToolError::invalid(format!(
            "offset {offset} is beyond the end of the file ({total} lines)"
        )));
    }
    let limit = input.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let start = offset;
    let end = (start + limit).min(total);

    let mut out = String::new();
    let mut truncated = false;
    for (index, line) in lines.iter().enumerate().take(end).skip(start) {
        let rendered = if line.chars().count() > MAX_LINE_LENGTH {
            truncated = true;
            let mut clipped: String = line.chars().take(MAX_LINE_LENGTH).collect();
            clipped.push_str("… [line truncated]");
            clipped
        } else {
            (*line).to_string()
        };
        out.push_str(&format!("{:>5}| {}\n", index + 1, rendered));
    }
    if end < total {
        truncated = true;
        out.push_str(&format!(
            "\n[Showing lines {}-{} of {}. Use offset={} to continue.]\n",
            start + 1,
            end,
            total,
            end
        ));
    }
    if out.len() > MAX_BYTES {
        out.truncate(MAX_BYTES);
        out.push_str("\n[output truncated at 50 KB]\n");
        truncated = true;
    }

    let relative = context.relative(&path);
    Ok(ToolOutput::new(
        format!("Read {relative}"),
        out,
        json!({
            "filePath": relative,
            "totalLines": total,
            "truncated": truncated,
        }),
    ))
}

fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.contains(&0u8) {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|byte| {
            !(byte.is_ascii_graphic() || byte.is_ascii_whitespace() || **byte >= 0x80)
        })
        .count();
    non_text * 100 / sample.len().max(1) > 5
}

fn detect_image(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => {
            if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                "image/png"
            } else if bytes.starts_with(b"\xff\xd8\xff") {
                "image/jpeg"
            } else if bytes.starts_with(b"GIF8") {
                "image/gif"
            } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
                "image/webp"
            } else {
                return None;
            }
        }
    };
    Some(mime)
}

fn image_attachment(context: &ToolContext, path: &Path, mime: &str, bytes: Vec<u8>) -> ToolOutput {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let relative = context.relative(path);
    ToolOutput::new(
        format!("Read image {relative}"),
        format!("Image {relative} ({mime}, {} bytes) attached.", bytes.len()),
        json!({
            "filePath": relative,
            "attachment": {
                "type": "file",
                "mime": mime,
                "url": format!("data:{mime};base64,{encoded}"),
            }
        }),
    )
}
