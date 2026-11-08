use rand::Rng;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Generates an OpenCode-shaped identifier: `prefix_` plus 26 base62 chars.
pub fn prefixed(prefix: &str) -> String {
    let mut rng = rand::thread_rng();
    let mut out = String::with_capacity(prefix.len() + 1 + 26);
    out.push_str(prefix);
    out.push('_');
    for _ in 0..26 {
        let idx = rng.gen_range(0..ALPHABET.len());
        out.push(ALPHABET[idx] as char);
    }
    out
}

pub fn session() -> String {
    prefixed("ses")
}

pub fn message() -> String {
    prefixed("msg")
}

pub fn permission() -> String {
    prefixed("per")
}

pub fn event() -> String {
    prefixed("evt")
}

pub fn part() -> String {
    prefixed("prt")
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Stable 40-hex project id derived from the canonical directory.
pub fn project_id(directory: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(directory.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(40);
    for byte in &digest[..20] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
