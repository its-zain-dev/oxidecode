use crate::jsonc;
use crate::paths::Paths;
use crate::util::expand_vars;
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// OpenCode V2 compatible configuration (opencode.json / opencode.jsonc).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: Option<String>,
    pub default_agent: Option<String>,
    pub shell: Option<String>,
    pub username: Option<String>,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// V1 compatibility: `provider` (singular).
    pub provider: BTreeMap<String, ProviderConfig>,
    pub agents: BTreeMap<String, AgentConfig>,
    pub commands: BTreeMap<String, CommandConfig>,
    pub permissions: Vec<PermissionRule>,
    /// V1 compatibility: object-style permissions.
    pub permission: Value,
    pub instructions: Vec<String>,
    pub skills: Vec<String>,
    pub mcp: Value,
    pub tool_output: ToolOutput,
    pub compaction: Value,
    pub experimental: Value,
    pub session: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: None,
            default_agent: None,
            shell: None,
            username: None,
            providers: BTreeMap::new(),
            provider: BTreeMap::new(),
            agents: BTreeMap::new(),
            commands: BTreeMap::new(),
            permissions: Vec::new(),
            permission: Value::Null,
            instructions: Vec::new(),
            skills: Vec::new(),
            mcp: Value::Null,
            tool_output: ToolOutput::default(),
            compaction: Value::Null,
            experimental: Value::Null,
            session: Value::Null,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub name: Option<String>,
    pub models: BTreeMap<String, ModelConfig>,
    pub settings: ProviderSettings,
    /// V1 compatibility.
    pub options: ProviderSettings,
    pub npm: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ProviderConfig {
    pub fn api_key(&self) -> Option<&str> {
        self.settings
            .api_key
            .as_deref()
            .or(self.options.api_key.as_deref())
            .or_else(|| self.extra.get("apiKey").and_then(Value::as_str))
            .or_else(|| self.extra.get("api_key").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
    }

    pub fn base_url(&self) -> Option<&str> {
        self.settings
            .base_url
            .as_deref()
            .or(self.options.base_url.as_deref())
            .or_else(|| self.extra.get("baseURL").and_then(Value::as_str))
            .or_else(|| self.extra.get("base_url").and_then(Value::as_str))
            .or_else(|| self.extra.get("api").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
    }

    pub fn headers(&self) -> &BTreeMap<String, String> {
        if self.settings.headers.is_empty() {
            &self.options.headers
        } else {
            &self.settings.headers
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderSettings {
    #[serde(rename = "apiKey", alias = "api_key")]
    pub api_key: Option<String>,
    #[serde(rename = "baseURL", alias = "base_url", alias = "api")]
    pub base_url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub transport: Option<String>,
    pub timeout: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    #[serde(rename = "modelID")]
    pub model_id: Option<String>,
    pub name: Option<String>,
    pub family: Option<String>,
    pub limit: Option<ModelLimit>,
    pub settings: Value,
    pub variants: Value,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub attachment: Option<bool>,
    pub temperature: Option<bool>,
    #[serde(rename = "cost")]
    pub cost: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelLimit {
    pub context: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub description: Option<String>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub system: Option<String>,
    pub prompt: Option<String>,
    pub hidden: Option<bool>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub permissions: Option<Vec<PermissionRule>>,
    pub tools: BTreeMap<String, bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandConfig {
    pub description: Option<String>,
    pub template: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub action: String,
    pub resource: String,
    pub effect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOutput {
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl Default for ToolOutput {
    fn default() -> Self {
        Self {
            max_lines: 2000,
            max_bytes: 51200,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDocument {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedConfig {
    pub config: Config,
    pub documents: Vec<ConfigDocument>,
    /// Directory the configuration was resolved for (the project root / cwd).
    pub directory: PathBuf,
    /// Provider credentials loaded from OpenCode/OxideCode `auth.json` and SQLite stores.
    pub credentials: BTreeMap<String, String>,
    /// Environment variables loaded from `.env` files in the project hierarchy.
    pub dotenv: BTreeMap<String, String>,
}

/// Candidate configuration files from lowest to highest precedence.
fn candidate_paths(paths: &Paths, directory: &Path) -> Vec<(String, PathBuf)> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    for name in ["opencode.jsonc", "opencode.json"] {
        let path = paths.opencode_config.join(name);
        if path.is_file() {
            candidates.push(("global".into(), path));
        }
    }
    for name in ["opencode.jsonc", "opencode.json"] {
        let path = paths.config.join(name);
        if path.is_file() {
            candidates.push(("global".into(), path));
        }
    }

    let mut chain: Vec<PathBuf> = Vec::new();
    let mut current = Some(directory.to_path_buf());
    while let Some(dir) = current {
        chain.push(dir.clone());
        current = dir.parent().map(|parent| parent.to_path_buf());
    }
    chain.reverse();

    for dir in &chain {
        for name in ["opencode.jsonc", "opencode.json"] {
            let path = dir.join(name);
            if path.is_file() {
                candidates.push(("project".into(), path));
            }
        }
    }
    for dir in &chain {
        for name in ["opencode.jsonc", "opencode.json"] {
            let path = dir.join(".opencode").join(name);
            if path.is_file() {
                candidates.push(("project".into(), path));
            }
        }
    }

    for base in ["/etc/opencode", "/data/data/com.termux/files/usr/etc/opencode"] {
        for name in ["opencode.jsonc", "opencode.json"] {
            let path = Path::new(base).join(name);
            if path.is_file() {
                candidates.push(("system".into(), path));
                break;
            }
        }
    }

    candidates
}

fn load_dotenv(directory: &Path) -> BTreeMap<String, String> {
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut current = Some(directory.to_path_buf());
    while let Some(dir) = current {
        chain.push(dir.clone());
        current = dir.parent().map(|parent| parent.to_path_buf());
    }
    chain.reverse();

    let mut vars = BTreeMap::new();
    for dir in chain {
        let env_file = dir.join(".env");
        let Ok(raw) = std::fs::read_to_string(&env_file) else {
            continue;
        };
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let line_body = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
            let Some((key, value)) = line_body.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let mut value = value.trim();
            if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            {
                value = &value[1..value.len() - 1];
            }
            if !value.is_empty() {
                vars.insert(key.to_string(), value.to_string());
            }
        }
    }
    vars
}

fn extract_secret_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Object(map) => {
            if let Some(inner) = map.get("value") {
                if let Some(secret) = extract_secret_from_value(inner) {
                    return Some(secret);
                }
            }
            for field in ["key", "apiKey", "api_key", "access", "access_token", "token", "secret"] {
                if let Some(s) = map.get(field).and_then(Value::as_str) {
                    if !s.trim().is_empty() {
                        return Some(s.trim().to_string());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_auth_json_value(value: &Value, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(map) => {
            // Handle `{ "data": [ Credential.Entry, ... ] }` or `{ "credentials": [...] }`
            for list_key in ["data", "credentials", "accounts"] {
                if let Some(Value::Array(items)) = map.get(list_key) {
                    parse_auth_json_array(items, out);
                }
            }
            for (provider_id, entry) in map {
                if matches!(provider_id.as_str(), "data" | "credentials" | "accounts" | "$schema") {
                    continue;
                }
                if let Some(secret) = extract_secret_from_value(entry) {
                    out.insert(provider_id.to_string(), secret);
                }
            }
        }
        Value::Array(items) => {
            parse_auth_json_array(items, out);
        }
        _ => {}
    }
}

fn parse_auth_json_array(items: &[Value], out: &mut BTreeMap<String, String>) {
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let active = obj.get("active").and_then(Value::as_bool).unwrap_or(true);
        let provider_id = obj
            .get("integrationID")
            .or_else(|| obj.get("integration_id"))
            .or_else(|| obj.get("providerID"))
            .or_else(|| obj.get("provider_id"))
            .or_else(|| obj.get("provider"))
            .or_else(|| obj.get("id"))
            .and_then(Value::as_str);
        let Some(provider_id) = provider_id.filter(|id| !id.is_empty()) else {
            continue;
        };
        if !active && out.contains_key(provider_id) {
            continue;
        }
        if let Some(secret) = extract_secret_from_value(item) {
            out.insert(provider_id.to_string(), secret);
        }
    }
}

fn load_sqlite_credentials(db_path: &Path, out: &mut BTreeMap<String, String>) {
    if !db_path.is_file() {
        return;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = rusqlite::Connection::open_with_flags(db_path, flags) else {
        return;
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(250));

    // Check `kv` table (used by OxideCode and some OpenCode stores)
    if let Ok(mut stmt) = conn.prepare("SELECT key, value FROM kv") {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            for row in rows.flatten() {
                let (key, val) = row;
                if key == "credentials" || key == "auth" || key == "auth.json" {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&val) {
                        parse_auth_json_value(&parsed, out);
                    }
                } else if let Some(provider) = key.strip_prefix("credential:") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&val) {
                        if let Some(secret) = extract_secret_from_value(&parsed) {
                            out.insert(provider.to_string(), secret);
                        }
                    } else if !val.trim().is_empty() {
                        out.insert(provider.to_string(), val.trim().to_string());
                    }
                }
            }
        }
    }

    // Check OpenCode V2 `credential` / `credentials` tables if present
    for query in [
        "SELECT integration_id, data_json FROM credential",
        "SELECT integration_id, value FROM credential",
        "SELECT integration_id, data_json FROM credentials",
        "SELECT integration_id, value FROM credentials",
        "SELECT data_json FROM credential",
        "SELECT data_json FROM credentials",
    ] {
        let Ok(mut stmt) = conn.prepare(query) else {
            continue;
        };
        let col_count = stmt.column_count();
        if col_count == 2 {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for (provider_id, raw) in rows.flatten() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&raw) {
                        if let Some(secret) = extract_secret_from_value(&parsed) {
                            out.insert(provider_id, secret);
                        }
                    } else if !raw.trim().is_empty() {
                        out.insert(provider_id, raw.trim().to_string());
                    }
                }
            }
        } else if col_count == 1 {
            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                for raw in rows.flatten() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&raw) {
                        parse_auth_json_value(&parsed, out);
                    }
                }
            }
        }
    }
}

fn load_credentials(paths: &Paths) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    load_sqlite_credentials(&paths.opencode_database(), &mut out);
    load_sqlite_credentials(&paths.database(), &mut out);

    for path in paths.auth_files() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let cleaned = jsonc::strip(&raw);
        let Ok(value) = serde_json::from_str::<Value>(&cleaned) else {
            continue;
        };
        parse_auth_json_value(&value, &mut out);
    }
    out
}

fn deep_merge(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                match target.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, source) => *target = source,
    }
}

fn expand_value(value: &mut Value, base_dir: &Path, home: &Path) {
    match value {
        Value::String(text) => {
            if text.contains("{env:") || text.contains("{file:") {
                *text = expand_vars(text, base_dir, home);
            }
        }
        Value::Array(items) => {
            for item in items {
                expand_value(item, base_dir, home);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                expand_value(item, base_dir, home);
            }
        }
        _ => {}
    }
}

impl LoadedConfig {
    pub fn load(paths: &Paths, directory: &Path) -> Self {
        let directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        let dotenv = load_dotenv(&directory);
        let credentials = load_credentials(paths);
        let candidates = candidate_paths(paths, &directory);
        let mut merged = Value::Object(serde_json::Map::new());
        let mut documents = Vec::new();

        for (kind, path) in candidates {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let cleaned = jsonc::strip(&raw);
            let Ok(mut value) = serde_json::from_str::<Value>(&cleaned) else {
                tracing::warn!("ignoring invalid config {}", path.display());
                continue;
            };
            let base_dir = path.parent().unwrap_or(Path::new("."));
            expand_value(&mut value, base_dir, &paths.home);
            deep_merge(&mut merged, value);
            documents.push(ConfigDocument {
                kind,
                path: path.display().to_string(),
            });
        }

        let mut config: Config = serde_json::from_value(merged).unwrap_or_default();
        if config.shell.as_deref().unwrap_or("").is_empty() {
            config.shell = Some(
                std::env::var("SHELL").unwrap_or_else(|_| "/system/bin/sh".to_string()),
            );
        }
        if config.default_agent.as_deref().unwrap_or("").is_empty() {
            config.default_agent = Some("build".to_string());
        }
        Self {
            config,
            documents,
            directory,
            credentials,
            dotenv,
        }
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.config
            .providers
            .get(id)
            .or_else(|| self.config.provider.get(id))
            .or_else(|| {
                if id == "opencode" {
                    self.config
                        .providers
                        .get("opencode-zen")
                        .or_else(|| self.config.provider.get("opencode-zen"))
                        .or_else(|| self.config.providers.get("opencode-go"))
                        .or_else(|| self.config.provider.get("opencode-go"))
                } else {
                    None
                }
            })
    }

    /// Returns a stored credential for `id` from `auth.json` or `opencode.db`.
    pub fn credential(&self, id: &str) -> Option<&str> {
        self.credentials
            .get(id)
            .or_else(|| {
                if id == "opencode" {
                    self.credentials
                        .get("opencode-zen")
                        .or_else(|| self.credentials.get("opencode-go"))
                        .or_else(|| self.credentials.get("console"))
                } else {
                    None
                }
            })
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Looks up an environment variable in the process environment, falling back
    /// to variables parsed from project `.env` files.
    pub fn env_var(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.dotenv.get(key).cloned().filter(|v| !v.is_empty()))
    }

    /// Resolves `<provider>/<model>[#variant]`.
    pub fn parse_model(&self, value: &str) -> Option<(String, String, Option<String>)> {
        let (provider, rest) = value.split_once('/')?;
        let (model, variant) = match rest.split_once('#') {
            Some((model, variant)) if !variant.is_empty() => {
                (model.to_string(), Some(variant.to_string()))
            }
            _ => (rest.to_string(), None),
        };
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some((provider.to_string(), model, variant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_objects_and_replace_arrays() {
        let mut a = serde_json::json!({"a": {"b": 1}, "list": [1]});
        let b = serde_json::json!({"a": {"c": 2}, "list": [2, 3]});
        deep_merge(&mut a, b);
        assert_eq!(a["a"]["b"], 1);
        assert_eq!(a["a"]["c"], 2);
        assert_eq!(a["list"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parses_auth_json_variants() {
        let raw = serde_json::json!({
            "opencode": { "type": "api", "key": "oc_sk_test123" },
            "anthropic": { "type": "oauth", "access": "ant_oauth_token" },
            "openai": "sk-direct"
        });
        let mut map = BTreeMap::new();
        parse_auth_json_value(&raw, &mut map);
        assert_eq!(map.get("opencode").map(String::as_str), Some("oc_sk_test123"));
        assert_eq!(map.get("anthropic").map(String::as_str), Some("ant_oauth_token"));
        assert_eq!(map.get("openai").map(String::as_str), Some("sk-direct"));
    }
}
