//! Browser entry point: landing page, password login and session cookies.
//!
//! The HTTP API is protected with HTTP Basic authentication, which a browser
//! cannot send until the server answers with a challenge. Rather than leaving
//! people staring at a JSON `403`/`401` when they open the printed server URL,
//! the root page renders a small status dashboard and offers a password form.
//! A successful login stores an opaque session token in an `HttpOnly` cookie
//! that the authentication middleware accepts alongside Basic/Bearer secrets.

use crate::state::{AppState, SESSION_TTL_MS};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use oxidecode_core::auth;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;

/// Cookie name used for browser sessions.
pub const SESSION_COOKIE: &str = "oxidecode_session";

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

/// Returns true when the request carries valid credentials, either as an
/// `Authorization` header or as a browser session cookie.
pub fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let secret = header.and_then(auth::basic_password).or_else(|| {
        header
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_string)
    });
    if secret
        .map(|secret| state.authorize(secret.trim()))
        .unwrap_or(false)
    {
        return true;
    }
    cookie_token(headers)
        .map(|token| state.verify_session_token(&token))
        .unwrap_or(false)
}

/// `GET /` — public status page. Viewing it never requires credentials, but
/// account details and the session list only appear once logged in.
pub async fn home(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let signed_in = authorized(&state, &headers);
    let body = page(&state, signed_in, params.get("error").map(String::as_str));
    let mut response = Html(body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `POST /auth/login` — validates the service password and starts a browser
/// session. Invalid passwords bounce back to the landing page with an error.
pub async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    if !state.verify(form.password.trim()) {
        return Redirect::to("/?error=invalid").into_response();
    }
    let token = state.create_session_token();
    let mut response = Redirect::to("/").into_response();
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL_MS / 1000
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// `POST /auth/logout` — forgets the browser session.
pub async fn logout() -> Response {
    let mut response = Redirect::to("/").into_response();
    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// `GET /favicon.ico` — keeps browser logs quiet without shipping an asset.
pub async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Builds the `Set-Cookie` value used by browser pairing.
pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL_MS / 1000
    )
}

/// True when the caller is a browser navigating to the pairing link.
pub fn prefers_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/html"))
        .unwrap_or(false)
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name.trim() == SESSION_COOKIE).then(|| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

fn page(state: &AppState, signed_in: bool, error: Option<&str>) -> String {
    let mut html = String::new();
    let _ = write!(
        html,
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OxideCode</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    background: radial-gradient(circle at top, #1b1b24 0%, #0a0a0a 60%);
    color: #e8e8ea; font: 15px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 24px;
  }}
  main {{
    width: min(680px, 100%); background: #111116; border: 1px solid #262630;
    border-radius: 14px; padding: 28px; box-shadow: 0 20px 60px rgba(0,0,0,.45);
  }}
  h1 {{ margin: 0; font-size: 24px; letter-spacing: -.02em; color: #fab283; }}
  h2 {{ margin: 26px 0 10px; font-size: 13px; text-transform: uppercase;
       letter-spacing: .12em; color: #8a8a96; }}
  p {{ margin: 8px 0; color: #b9b9c4; }}
  .sub {{ margin: 4px 0 22px; color: #777783; }}
  .status {{ display: flex; align-items: center; gap: 9px; color: #9ef0a8;
            font-weight: 600; margin: 0 0 18px; }}
  .dot {{ width: 9px; height: 9px; border-radius: 50%; background: #55d66b;
          box-shadow: 0 0 12px #55d66b; }}
  dl {{ display: grid; grid-template-columns: max-content 1fr; gap: 8px 18px;
        margin: 0; padding: 16px; background: #0c0c10; border-radius: 9px;
        border: 1px solid #1f1f28; }}
  dt {{ color: #7d7d89; }}
  dd {{ margin: 0; color: #dedee5; overflow-wrap: anywhere; }}
  a {{ color: #8ab4ff; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  code {{ background: #1c1c24; padding: 2px 6px; border-radius: 5px; color: #f0c08a; }}
  .links {{ display: flex; flex-wrap: wrap; gap: 10px 18px; margin-top: 14px; }}
  .error {{ background: #3a1518; border: 1px solid #6f252b; color: #ffb3b8;
            padding: 10px 12px; border-radius: 8px; }}
  form.login {{ display: grid; gap: 12px; margin-top: 18px; }}
  input[type=password] {{
    width: 100%; padding: 12px 14px; border-radius: 9px; border: 1px solid #343440;
    background: #0b0b0f; color: #f2f2f5; font: inherit; outline: none;
  }}
  input[type=password]:focus {{ border-color: #fab283; }}
  button {{
    padding: 12px 16px; border-radius: 9px; border: 0; cursor: pointer;
    background: #fab283; color: #201407; font: inherit; font-weight: 700;
  }}
  button:hover {{ background: #ffc39c; }}
  button.ghost {{ background: transparent; color: #9a9aa6;
                  border: 1px solid #30303b; font-weight: 500; }}
  button.ghost:hover {{ color: #dedee5; border-color: #4a4a58; background: #17171e; }}
  .row {{ display: flex; justify-content: space-between; align-items: center;
          gap: 16px; flex-wrap: wrap; margin-top: 20px; }}
  .hint {{ color: #6e6e7a; font-size: 13px; margin-top: 18px; }}
  ul.sessions {{ list-style: none; margin: 0; padding: 0; display: grid; gap: 7px; }}
  ul.sessions li {{ display: flex; justify-content: space-between; gap: 16px;
                    padding: 10px 12px; background: #0c0c10; border: 1px solid #1f1f28;
                    border-radius: 8px; }}
  ul.sessions span {{ overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
  ul.sessions code {{ font-size: 11px; color: #6e6e7a; background: none; padding: 0; }}
  .empty {{ color: #6e6e7a; }}
</style>
</head>
<body>
<main>
  <h1>OxideCode</h1>
  <p class="sub">OpenCode-compatible agent server</p>
"#
    );

    if signed_in {
        let directory = state.default_directory();
        let uptime = human_duration(state.started().elapsed().as_secs());
        let _ = write!(
            html,
            r#"  <p class="status"><span class="dot"></span> Server is running</p>
  <dl>
    <dt>Version</dt><dd>{version}</dd>
    <dt>Uptime</dt><dd>{uptime}</dd>
    <dt>Hostname</dt><dd>{hostname}</dd>
    <dt>Address</dt><dd>127.0.0.1:{port}</dd>
    <dt>Working directory</dt><dd>{directory}</dd>
  </dl>
  <div class="links">
    <a href="/api/info">Server info (JSON)</a>
    <a href="/api/session">Sessions (JSON)</a>
    <a href="/api/model">Models (JSON)</a>
    <a href="/api/event">Event stream (SSE)</a>
  </div>
"#,
            version = escape(state.version()),
            uptime = escape(&uptime),
            hostname = escape(state.hostname()),
            port = state.port(),
            directory = escape(&directory.display().to_string()),
        );

        let sessions = state
            .store()
            .list_sessions(
                None,
                Some(&directory.display().to_string()),
                None,
                8,
                false,
                "desc",
            )
            .unwrap_or_default();
        let _ = write!(html, "  <h2>Recent sessions</h2>\n");
        if sessions.is_empty() {
            let _ = write!(
                html,
                "  <p class=\"empty\">No sessions in this directory yet.</p>\n"
            );
        } else {
            let _ = write!(html, "  <ul class=\"sessions\">\n");
            for session in sessions {
                let title = if session.title.trim().is_empty() {
                    session.id.clone()
                } else {
                    session.title.clone()
                };
                let _ = write!(
                    html,
                    "    <li><span>{}</span><code>{}</code></li>\n",
                    escape(&title),
                    escape(&session.id)
                );
            }
            let _ = write!(html, "  </ul>\n");
        }

        let _ = write!(
            html,
            r#"  <div class="row">
    <form method="post" action="/auth/logout"><button class="ghost" type="submit">Sign out</button></form>
  </div>
"#
        );
    } else {
        let _ = write!(
            html,
            r#"  <p class="status"><span class="dot"></span> Server is running</p>
  <dl>
    <dt>Version</dt><dd>{version}</dd>
    <dt>Address</dt><dd>127.0.0.1:{port}</dd>
  </dl>
  <p style="margin-top:18px">This server is protected by a service password. Sign in to view its details and sessions.</p>
"#,
            version = escape(state.version()),
            port = state.port(),
        );
        if error == Some("invalid") {
            let _ = write!(
                html,
                "  <p class=\"error\">That password is not correct. Try again.</p>\n"
            );
        }
        let _ = write!(
            html,
            r#"  <form class="login" method="post" action="/auth/login">
    <input type="password" name="password" placeholder="Service password" autocomplete="current-password" autofocus required>
    <button type="submit">Sign in</button>
  </form>
  <p class="hint">Find the password with <code>oxidecode service get password</code> (or <code>opencode service get password</code>).</p>
"#
        );
    }

    html.push_str("</main>\n</body>\n</html>\n");
    html
}

fn human_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(
            escape("<script>alert('x') & \"y\"</script>"),
            "&lt;script&gt;alert(&#39;x&#39;) &amp; &quot;y&quot;&lt;/script&gt;"
        );
    }

    #[test]
    fn reads_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; oxidecode_session=abc123; other=1"),
        );
        assert_eq!(cookie_token(&headers).as_deref(), Some("abc123"));
    }

    #[test]
    fn formats_uptime() {
        assert_eq!(human_duration(3), "3s");
        assert_eq!(human_duration(125), "2m 5s");
        assert_eq!(human_duration(7_320), "2h 2m");
        assert_eq!(human_duration(90_000), "1d 1h");
    }
}
