use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCredentials {
    #[serde(default)]
    pub password: String,
}

/// Loads the service password, generating a 43-char base64url secret on first use.
pub fn load_or_create_password(path: &Path) -> io::Result<String> {
    if let Ok(raw) = fs::read_to_string(path) {
        if let Ok(credentials) = serde_json::from_str::<ServiceCredentials>(&raw) {
            if !credentials.password.is_empty() {
                return Ok(credentials.password);
            }
        }
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let password = URL_SAFE_NO_PAD.encode(bytes);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(&ServiceCredentials {
        password: password.clone(),
    })
    .map_err(io::Error::other)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    use std::io::Write;
    file.write_all(&body)?;
    file.sync_all()?;
    Ok(password)
}

/// Constant-time comparison for secrets.
pub fn secret_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Parses an `Authorization` header and returns the password when it authenticates
/// as user `opencode` (the username OpenCode's own clients use).
pub fn basic_password(header: &str) -> Option<String> {
    let encoded = header.strip_prefix("Basic ").or_else(|| header.strip_prefix("basic "))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(encoded.trim()))
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    if user == "opencode" || user == "oxidecode" {
        Some(password.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_basic_auth() {
        let value = base64::engine::general_purpose::STANDARD.encode("opencode:secret");
        assert_eq!(basic_password(&format!("Basic {value}")).as_deref(), Some("secret"));
    }
}
