use crate::jsonc;
use crate::paths::Paths;
use crate::util::expand_vars;
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
            .filter(|value| !value.is_empty())
    }

    pub fn base_url(&self) -> Option<&str> {
        self.settings
            .base_url
            .as_deref()
            .or(self.options.base_url.as_deref())
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
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "baseURL")]
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
        }
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.config.providers.get(id).or_else(|| self.config.provider.get(id))
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
}
