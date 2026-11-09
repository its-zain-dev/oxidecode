use crate::catalog::ResolvedModel;
use crate::openai::{build_body, process_sse_stream, reasoning_field_names, resolve_endpoint_url};
use crate::types::{ProviderRequest, ProviderStream};
use anyhow::{Context, Result};
use bytes::Bytes;
use serde_json::json;
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;

const FETCH_SCRIPT: &str = r#"
const fs = require('fs');
const req = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
try {
  let body = req.body;
  try { body = JSON.parse(req.body); } catch {}
  fs.writeFileSync('/data/data/com.termux/files/usr/tmp/oxidecode-last-req.json', JSON.stringify({url:req.url,headers:req.headers,body}));
} catch (e) { try { fs.writeFileSync('/data/data/com.termux/files/usr/tmp/oxidecode-last-req.err', String(e)); } catch {} }
try {
  const res = await fetch(req.url, { method: 'POST', headers: req.headers, body: req.body });
  if (!res.ok) {
    let text = '';
    try { text = await res.text(); } catch {}
    const msg = (text || res.statusText || 'HTTP error').slice(0, 4000);
    process.stdout.write('data: ' + JSON.stringify({error:{message:msg, status:res.status, type:'HTTPError'}}) + '\n\n');
    process.stdout.write('data: [DONE]\n\n');
    process.exit(0);
  }
  // Stream raw SSE bytes to stdout.
  if (res.body) {
    for await (const chunk of res.body) process.stdout.write(chunk);
  } else {
    const text = await res.text();
    process.stdout.write(text);
  }
} catch (e) {
  const msg = (e && e.message ? e.message : String(e)).slice(0, 4000);
  process.stdout.write('data: ' + JSON.stringify({error:{message:msg, type:'TransportError'}}) + '\n\n');
  process.stdout.write('data: [DONE]\n\n');
  process.exit(0);
}
"#;

fn opencode_bin() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("OPENCODE_BIN") {
        let p = std::path::PathBuf::from(p.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    for p in [
        "/data/data/com.termux/files/usr/bin/opencode",
        "/usr/local/bin/opencode",
        "/usr/bin/opencode",
    ] {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn can_use(model: &ResolvedModel) -> bool {
    let is_opencode = matches!(
        model.provider_id.as_str(),
        "opencode" | "opencode-zen" | "opencode-go"
    ) || model.base_url.contains("opencode.ai");
    is_opencode && opencode_bin().is_some()
}

fn hex_id(seed: u128, nbytes: usize) -> String {
    let mut x = seed;
    let mut out = String::with_capacity(nbytes * 2);
    for _ in 0..nbytes {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        out.push_str(&format!("{:02x}", (x >> 56) as u8));
    }
    out
}

fn build_headers(model: &ResolvedModel) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    headers.insert("Accept".into(), "*/*".into());
    headers.insert("Content-Type".into(), "application/json".into());
    headers.insert("User-Agent".into(), "opencode/stable/2.2.6/cli".into());
    headers.insert("x-opencode-client".into(), "cli".into());
    for (k, v) in &model.headers {
        headers.insert(k.clone(), v.clone());
    }
    if let Some(sid) = model
        .headers
        .get("x-opencode-session")
        .or_else(|| model.headers.get("x-session-id"))
        .cloned()
    {
        for k in [
            "x-opencode-session",
            "x-opencode-session-id",
            "x-session-affinity",
            "x-session-id",
        ] {
            headers.insert(k.into(), sid.clone());
        }
    }
    headers.entry("x-opencode-project".into()).or_insert_with(|| "oxidecode".into());
    // Mimic tracing headers sent by real OpenCode.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) ^ (std::process::id() as u128);
    headers.entry("traceparent".into()).or_insert_with(|| {
        format!("00-{}-{}-01", hex_id(seed, 16), hex_id(seed ^ 0x55, 8))
    });
    headers.entry("b3".into()).or_insert_with(|| {
        format!(
            "{}-{}-1-{}",
            hex_id(seed, 16),
            hex_id(seed ^ 0x55, 8),
            hex_id(seed ^ 0xaa, 8)
        )
    });
    if let Some(key) = &model.api_key {
        if !key.is_empty() && !headers.contains_key("Authorization") && !headers.contains_key("authorization") {
            headers.insert("Authorization".into(), format!("Bearer {key}"));
        }
    }
    headers
}

pub async fn stream(model: &ResolvedModel, request: ProviderRequest) -> Result<ProviderStream> {
    let bin = opencode_bin().context("OpenCode binary not found for Bun TLS transport")?;
    let url = resolve_endpoint_url(&model.base_url);
    let body = build_body(model, &request);
    let headers = build_headers(model);
    let req_json = json!({
        "url": url,
        "headers": headers,
        "body": body.to_string(),
    });
    // Debug dump
    if let Ok(dbg) = serde_json::to_string_pretty(&json!({"url":&url,"headers":&req_json["headers"],"body":&body})) {
        let _ = std::fs::write("/data/data/com.termux/files/usr/tmp/oxidecode-last-req.json", dbg);
    }

    let tmp = std::env::var("OXIDECODE_TMP")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let p = std::path::PathBuf::from("/data/data/com.termux/files/usr/tmp");
            if p.is_dir() { p } else { std::env::temp_dir() }
        });
    let _ = std::fs::create_dir_all(&tmp);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = format!("oxidecode-bun-{}-{}", std::process::id(), nonce);
    let script_path = tmp.join(format!("{base}.mjs"));
    let req_path = tmp.join(format!("{base}.json"));
    std::fs::write(&script_path, FETCH_SCRIPT).context("write bun fetch script")?;
    std::fs::write(&req_path, req_json.to_string()).context("write bun fetch request")?;

    let mut child = Command::new(&bin)
        .env("BUN_BE_BUN", "1")
        .arg(&script_path)
        .arg(&req_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn opencode bun transport: {}", bin.display()))?;

    let stdout = child.stdout.take().context("bun transport stdout")?;
    let mut stderr = child.stderr.take().context("bun transport stderr")?;

    // Drain stderr in background so the child can't block on a full pipe.
    tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
    });

    let reasoning_fields = reasoning_field_names(model.reasoning_field.as_deref());
    let byte_stream = async_stream::stream! {
        let mut reader = BufReader::with_capacity(16 * 1024, stdout);
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
                Ok(0) => break,
                Ok(n) => yield Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(&buf[..n])),
                Err(e) => {
                    yield Err(e);
                    break;
                }
            }
        }
        let status = child.wait().await;
        let _ = status;
        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_file(&req_path);
    };

    Ok(process_sse_stream(byte_stream, reasoning_fields))
}
