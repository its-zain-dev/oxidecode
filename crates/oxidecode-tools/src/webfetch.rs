use super::context::{ToolContext, ToolError, ToolOutput};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct Input {
    pub url: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

const MAX_BODY: usize = 5 * 1024 * 1024;

/// Reuses one pooled client instead of rebuilding a TLS stack for every fetch.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (compatible; OxideCode/0.1; +https://opencode.ai)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

pub async fn run(context: &ToolContext, input: Input) -> Result<ToolOutput, ToolError> {
    let url = input.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ToolError::invalid(
            "url must start with http:// or https://",
        ));
    }
    let format = input
        .format
        .as_deref()
        .unwrap_or("markdown")
        .to_ascii_lowercase();
    let timeout = input.timeout.unwrap_or(30).clamp(1, 120);

    let response = client()
        .get(&url)
        .timeout(Duration::from_secs(timeout))
        .header(
            "Accept",
            "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|error| ToolError::failed(format!("fetch {url}: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::failed(format!(
            "fetch {url}: HTTP {}",
            status.as_u16()
        )));
    }
    if response.content_length().unwrap_or(0) > MAX_BODY as u64 {
        return Err(ToolError::failed("response is larger than 5 MB"));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ToolError::failed(error.to_string()))?;
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(ToolError::failed("response is larger than 5 MB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();

    let rendered = match format.as_str() {
        "html" => body,
        "text" => html_to_text(&body),
        _ => html_to_markdown(&url, &body),
    };
    let (rendered, truncated) = context.truncate(&rendered);
    let title = format!("Fetched {url} ({})", status.as_u16());
    Ok(ToolOutput::new(
        title,
        rendered,
        json!({
            "url": url,
            "status": status.as_u16(),
            "contentType": content_type,
            "format": format,
            "truncated": truncated,
        }),
    ))
}

fn is_html(content_type_or_body: &str) -> bool {
    let lower = content_type_or_body.to_ascii_lowercase();
    lower.contains("<html") || lower.contains("<!doctype html")
}

fn html_to_text(html: &str) -> String {
    if !is_html(html) {
        return html.to_string();
    }
    let mut out = strip_blocks(html);
    // Turn block-level boundaries into newlines before stripping tags.
    for (from, to) in [
        ("<br>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("</p>", "\n\n"),
        ("</div>", "\n"),
        ("</li>", "\n"),
        ("</tr>", "\n"),
        ("</h1>", "\n\n"),
        ("</h2>", "\n\n"),
        ("</h3>", "\n\n"),
        ("</h4>", "\n\n"),
        ("</h5>", "\n\n"),
        ("</h6>", "\n\n"),
        ("</pre>", "\n\n"),
        ("</blockquote>", "\n\n"),
    ] {
        out = out.replace(from, to);
        out = out.replace(&from.to_ascii_uppercase(), to);
    }
    let out = strip_tags(&out);
    let out = decode_entities(&out);
    collapse_blank_lines(&out)
}

fn html_to_markdown(url: &str, html: &str) -> String {
    if !is_html(html) {
        return html.to_string();
    }
    let mut out = strip_blocks(html);

    // Links first so their text survives tag stripping.
    out = linkify(&out);
    for level in 1..=6u8 {
        let open = format!("<h{level}");
        let close = format!("</h{level}>");
        while let Some(start) = find_tag(&out, &open) {
            let Some(content_start) = out[start..].find('>').map(|offset| start + offset + 1)
            else {
                break;
            };
            let Some(end_offset) = out[content_start..].find(&close) else {
                break;
            };
            let content = strip_tags(&out[content_start..content_start + end_offset]);
            let hashes = "#".repeat(level as usize);
            let replacement = format!("\n\n{hashes} {}\n\n", decode_entities(content.trim()));
            out.replace_range(
                start..content_start + end_offset + close.len(),
                &replacement,
            );
        }
    }
    for (from, to) in [
        ("<br>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("</p>", "\n\n"),
        ("</div>", "\n"),
        ("</li>", "\n- "),
        ("</tr>", "\n"),
        ("</pre>", "\n```\n\n"),
        ("<pre>", "\n```\n"),
        ("</blockquote>", "\n\n"),
        ("<hr>", "\n---\n"),
        ("<hr/>", "\n---\n"),
    ] {
        out = out.replace(from, to);
        out = out.replace(&from.to_ascii_uppercase(), to);
    }
    let out = strip_tags(&out);
    let out = decode_entities(&out);
    let mut out = collapse_blank_lines(&out);
    out.push_str(&format!("\n\n[Source: {url}]"));
    out
}

fn find_tag(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

fn linkify(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(start) = lower[cursor..].find("<a ") {
        let start = cursor + start;
        out.push_str(&html[cursor..start]);
        let Some(open_end_offset) = lower[start..].find('>') else {
            out.push_str(&html[start..]);
            return out;
        };
        let open_end = start + open_end_offset;
        let tag = &html[start..open_end];
        let href = extract_attribute(tag, "href").unwrap_or_default();
        let Some(close_offset) = lower[open_end..].find("</a>") else {
            out.push_str(&html[start..]);
            return out;
        };
        let close = open_end + close_offset;
        let text = strip_tags(&html[open_end + 1..close]);
        let text = decode_entities(text.trim());
        if href.is_empty() {
            out.push_str(&text);
        } else if text.is_empty() || text == href {
            out.push_str(&href);
        } else {
            out.push_str(&format!("[{text}]({href})"));
        }
        cursor = close + 4;
    }
    out.push_str(&html[cursor..]);
    out
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(index) = lower[search..].find(name) {
        let index = search + index;
        let after = lower[index + name.len()..].trim_start();
        if let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            let quote = after.chars().next()?;
            if quote == '"' || quote == '\'' {
                let rest = &after[1..];
                let end = rest.find(quote)?;
                return Some(decode_entities(&rest[..end]));
            }
            let end = after
                .find(|ch: char| ch.is_whitespace() || ch == '>')
                .unwrap_or(after.len());
            return Some(decode_entities(&after[..end]));
        }
        search = index + name.len();
    }
    None
}

fn strip_blocks(html: &str) -> String {
    let mut out = html.to_string();
    for (open, close) in [
        ("<script", "</script>"),
        ("<style", "</style>"),
        ("<noscript", "</noscript>"),
        ("<svg", "</svg>"),
        ("<!--", "-->"),
    ] {
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(start) = lower.find(open) else {
                break;
            };
            let Some(end_offset) = lower[start..].find(close) else {
                out.truncate(start);
                break;
            };
            let end = start + end_offset + close.len();
            out.replace_range(start..end, " ");
        }
    }
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut inside = false;
    for ch in html.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(ch),
            _ => {}
        }
    }
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
        .replace("&copy;", "©");
    // Numeric entities.
    let mut result = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(start) = rest.find("&#") {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find(';');
        match end {
            Some(end) => {
                let number = tail[..end].trim_start_matches('x').trim_start_matches('X');
                let parsed = if tail.starts_with('x') || tail.starts_with('X') {
                    u32::from_str_radix(number, 16).ok()
                } else {
                    number.parse::<u32>().ok()
                };
                match parsed.and_then(char::from_u32) {
                    Some(ch) => result.push(ch),
                    None => result.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &tail[end + 1..];
            }
            None => {
                result.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    result.push_str(rest);
    out = result;
    out
}

fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0usize;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}
