use oxidecode_core::config::LoadedConfig;
use oxidecode_core::session::ModelRef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const CATALOG_JSON: &str = include_str!("catalog.json");

#[derive(Debug, Deserialize)]
struct RawCatalog {
    providers: BTreeMap<String, RawProvider>,
}

#[derive(Debug, Deserialize, Clone)]
struct RawProvider {
    #[allow(dead_code)]
    id: String,
    name: String,
    #[serde(default)]
    env: Vec<String>,
    api: Option<String>,
    npm: Option<String>,
    #[serde(default)]
    models: Vec<RawModel>,
}

#[derive(Debug, Deserialize, Clone)]
struct RawModel {
    id: String,
    name: String,
    family: Option<String>,
    #[serde(default)]
    attachment: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    tool_call: bool,
    #[serde(default)]
    temperature: bool,
    limit: Option<RawLimit>,
    cost: Option<Value>,
    #[serde(default = "default_status")]
    status: String,
    release_date: Option<String>,
    modalities: Option<Value>,
    interleaved: Option<Value>,
    provider: Option<RawProviderRef>,
    reasoning_options: Option<Value>,
}

fn default_status() -> String {
    "active".to_string()
}

#[derive(Debug, Deserialize, Clone)]
struct RawLimit {
    context: Option<u64>,
    #[serde(default)]
    output: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
struct RawProviderRef {
    npm: Option<String>,
    api: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimit {
    pub context: u64,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub family: String,
    pub name: String,
    pub compatibility: Value,
    pub package: String,
    pub settings: Value,
    pub capabilities: Value,
    pub variants: Value,
    pub time: Value,
    pub cost: Value,
    pub status: String,
    pub enabled: bool,
    pub limit: ModelLimit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    #[serde(rename = "integrationID")]
    pub integration_id: String,
    pub name: String,
    pub activation: String,
    pub package: String,
    pub settings: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    OpenAi,
    Anthropic,
    Mock,
}

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub provider_id: String,
    pub model_id: String,
    pub variant: Option<String>,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub protocol: Protocol,
    pub package: String,
}

impl ResolvedModel {
    pub fn display(&self) -> String {
        match &self.variant {
            Some(variant) if !variant.is_empty() => {
                format!("{}/{}#{}", self.provider_id, self.model_id, variant)
            }
            _ => format!("{}/{}", self.provider_id, self.model_id),
        }
    }
}

fn catalog() -> RawCatalog {
    serde_json::from_str(CATALOG_JSON).unwrap_or(RawCatalog {
        providers: BTreeMap::new(),
    })
}

fn catalog_providers() -> Vec<RawProvider> {
    catalog().providers.into_values().collect()
}

fn default_api(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("https://api.openai.com/v1"),
        "anthropic" => Some("https://api.anthropic.com/v1"),
        "google" => Some("https://generativelanguage.googleapis.com/v1beta"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "groq" => Some("https://api.groq.com/openai/v1"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "opencode" => Some("https://opencode.ai/inference/openai/v1"),
        "ollama" => Some("http://127.0.0.1:11434/v1"),
        "mock" => Some("mock://local"),
        _ => None,
    }
}

fn env_key(provider: &str, declared: &[String]) -> Option<String> {
    for key in declared {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    let fallback = match provider {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "google" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "groq" => "GROQ_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "xai" => "XAI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "opencode" => "OPENCODE_API_KEY",
        _ => return None,
    };
    std::env::var(fallback)
        .ok()
        .filter(|value| !value.is_empty())
}

fn provider_available(config: &LoadedConfig, id: &str, declared_env: &[String]) -> bool {
    if id == "mock" {
        return std::env::var("OXIDECODE_MOCK").map(|v| v != "0").unwrap_or(false);
    }
    if let Some(provider) = config.provider(id) {
        if provider.api_key().is_some() || provider.base_url().is_some() {
            return true;
        }
    }
    if id == "ollama"
        && (std::env::var("OLLAMA_HOST").ok().filter(|v| !v.is_empty()).is_some()
            || config.provider(id).is_some())
    {
        return true;
    }
    env_key(id, declared_env).is_some()
}

fn base_url(config: &LoadedConfig, id: &str, api: Option<&str>) -> String {
    if let Some(provider) = config.provider(id) {
        if let Some(url) = provider.base_url() {
            return url.trim_end_matches('/').to_string();
        }
    }
    if let Some(url) = api {
        return url.trim_end_matches('/').to_string();
    }
    if id == "ollama" {
        if let Ok(host) = std::env::var("OLLAMA_HOST") {
            if !host.is_empty() {
                let host = if host.starts_with("http") {
                    host
                } else {
                    format!("http://{host}")
                };
                return format!("{}/v1", host.trim_end_matches('/'));
            }
        }
    }
    default_api(id)
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/')
        .to_string()
}

fn date_ms(date: &str) -> i64 {
    // Dates are YYYY-MM-DD; a civil-days calculation avoids a time crate round trip.
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return 0;
    };
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400_000
}

fn capability_value(model: &RawModel) -> Value {
    let input = model
        .modalities
        .as_ref()
        .and_then(|value| value.get("input"))
        .and_then(Value::as_array)
        .map(|values| {
            let has = |name: &str| values.iter().any(|value| value.as_str() == Some(name));
            json!({
                "text": has("text"),
                "image": has("image"),
                "audio": has("audio"),
                "video": has("video"),
                "pdf": has("pdf"),
            })
        })
        .unwrap_or_else(|| json!({"text": true, "image": model.attachment, "audio": false, "video": false, "pdf": false}));
    json!({
        "tools": model.tool_call,
        "input": input,
        "output": {"text": true},
        "temperature": model.temperature,
        "reasoning": model.reasoning,
    })
}

fn cost_value(model: &RawModel) -> Value {
    let Some(cost) = model.cost.as_ref() else {
        return json!([]);
    };
    let input = cost.get("input").and_then(Value::as_f64).unwrap_or(0.0);
    let output = cost.get("output").and_then(Value::as_f64).unwrap_or(0.0);
    let read = cost.get("cache_read").and_then(Value::as_f64).unwrap_or(0.0);
    let write = cost.get("cache_write").and_then(Value::as_f64).unwrap_or(0.0);
    json!([{
        "input": input,
        "output": output,
        "cache": {"read": read, "write": write}
    }])
}

fn to_model_info(
    provider_id: &str,
    provider_package: &str,
    api: Option<&str>,
    api_key: Option<&str>,
    model: &RawModel,
    enabled: bool,
) -> ModelInfo {
    let limit = model.limit.as_ref();
    let context = limit.and_then(|value| value.context).unwrap_or(128_000);
    ModelInfo {
        id: model.id.clone(),
        model_id: model.id.clone(),
        provider_id: provider_id.to_string(),
        kind: "language".to_string(),
        family: model.family.clone().unwrap_or_else(|| provider_id.to_string()),
        name: model.name.clone(),
        compatibility: json!({
            "reasoningField": model.interleaved.as_ref().and_then(|v| v.get("field")).and_then(Value::as_str),
            "reasoningOptions": model.reasoning_options,
        }),
        package: model
            .provider
            .as_ref()
            .and_then(|value| value.npm.clone())
            .unwrap_or_else(|| provider_package.to_string()),
        settings: json!({
            "apiKey": api_key,
            "baseURL": base_url_for(model, api),
            "provider": provider_id,
        }),
        capabilities: capability_value(model),
        variants: json!([]),
        time: json!({"released": model.release_date.as_deref().map(date_ms).unwrap_or(0)}),
        cost: cost_value(model),
        status: model.status.clone(),
        enabled,
        limit: ModelLimit {
            context,
            input: context,
            output: limit.and_then(|value| value.output).unwrap_or(32_000),
        },
        description: None,
    }
}

fn base_url_for(model: &RawModel, api: Option<&str>) -> Option<String> {
    model
        .provider
        .as_ref()
        .and_then(|value| value.api.clone())
        .or_else(|| api.map(str::to_string))
}

/// Returns every model in the bundled catalog plus models declared in the
/// user's configuration, annotated with availability.
pub fn models(config: &LoadedConfig) -> Vec<ModelInfo> {
    let catalog = catalog();
    let mut out: Vec<ModelInfo> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

    for (id, provider) in &catalog.providers {
        let enabled = provider_available(config, id, &provider.env);
        let api_key = env_key(id, &provider.env)
            .or_else(|| config.provider(id).and_then(|p| p.api_key().map(str::to_string)));
        for model in &provider.models {
            seen.insert((id.clone(), model.id.clone()));
            out.push(to_model_info(
                id,
                provider.npm.as_deref().unwrap_or("@ai-sdk/openai-compatible"),
                provider.api.as_deref(),
                api_key.as_deref(),
                model,
                enabled,
            ));
        }
    }

    // Locally declared providers and models.
    for (id, provider) in config.config.providers.iter().chain(config.config.provider.iter()) {
        let enabled = provider.api_key().is_some() || provider.base_url().is_some();
        let api_key = provider.api_key().map(str::to_string).or_else(|| {
            env_key(id, &[])
        });
        for (model_id, model) in &provider.models {
            let id_final = model.model_id.clone().unwrap_or_else(|| model_id.clone());
            if !seen.insert((id.clone(), id_final.clone())) {
                continue;
            }
            let context = model
                .limit
                .as_ref()
                .and_then(|limit| limit.context)
                .unwrap_or(128_000);
            out.push(ModelInfo {
                id: id_final.clone(),
                model_id: id_final,
                provider_id: id.clone(),
                kind: "language".to_string(),
                family: model.family.clone().unwrap_or_else(|| id.clone()),
                name: model.name.clone().unwrap_or_else(|| model_id.clone()),
                compatibility: json!({}),
                package: provider
                    .npm
                    .clone()
                    .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string()),
                settings: json!({
                    "apiKey": api_key,
                    "baseURL": provider.base_url(),
                    "provider": id,
                }),
                capabilities: json!({
                    "tools": model.tool_call.unwrap_or(true),
                    "input": {"text": true, "image": model.attachment.unwrap_or(false)},
                    "output": {"text": true},
                    "temperature": model.temperature.unwrap_or(true),
                }),
                variants: model.variants.clone(),
                time: json!({"released": 0}),
                cost: model.cost.clone(),
                status: "active".to_string(),
                enabled,
                limit: ModelLimit {
                    context,
                    input: context,
                    output: model
                        .limit
                        .as_ref()
                        .and_then(|limit| limit.output)
                        .unwrap_or(32_000),
                },
                description: model.settings.as_str().map(str::to_string),
            });
        }
    }

    // Always provide a mock model so the application is testable offline.
    let mock_enabled = std::env::var("OXIDECODE_MOCK").map(|v| v != "0").unwrap_or(false)
        || out.iter().all(|model| !model.enabled);
    if mock_enabled && seen.insert(("mock".into(), "mock-echo".into())) {
        out.push(ModelInfo {
            id: "mock-echo".into(),
            model_id: "mock-echo".into(),
            provider_id: "mock".into(),
            kind: "language".into(),
            family: "mock".into(),
            name: "OxideCode Mock".into(),
            compatibility: json!({}),
            package: "mock".into(),
            settings: json!({"provider": "mock"}),
            capabilities: json!({
                "tools": true,
                "input": {"text": true, "image": false},
                "output": {"text": true},
                "temperature": true,
            }),
            variants: json!([]),
            time: json!({"released": 0}),
            cost: json!([]),
            status: "active".into(),
            enabled: true,
            limit: ModelLimit {
                context: 128_000,
                input: 128_000,
                output: 8_192,
            },
            description: Some("Deterministic local provider used for tests.".into()),
        });
    }

    out.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| a.provider_id.cmp(&b.provider_id))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn providers(config: &LoadedConfig) -> Vec<ProviderInfo> {
    let catalog = catalog();
    let mut out = Vec::new();
    for (id, provider) in &catalog.providers {
        let enabled = provider_available(config, id, &provider.env);
        out.push(ProviderInfo {
            id: id.clone(),
            integration_id: id.clone(),
            name: provider.name.clone(),
            activation: if enabled { "enabled".into() } else { "available".into() },
            package: provider
                .npm
                .clone()
                .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string()),
            settings: json!({
                "baseURL": config.provider(id).and_then(|p| p.base_url()).or(provider.api.as_deref()),
            }),
        });
    }
    for (id, provider) in config.config.providers.iter().chain(config.config.provider.iter()) {
        if out.iter().any(|info| &info.id == id) {
            continue;
        }
        out.push(ProviderInfo {
            id: id.clone(),
            integration_id: id.clone(),
            name: provider.name.clone().unwrap_or_else(|| id.clone()),
            activation: if provider.api_key().is_some() || provider.base_url().is_some() {
                "enabled".into()
            } else {
                "available".into()
            },
            package: provider
                .npm
                .clone()
                .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string()),
            settings: json!({"baseURL": provider.base_url()}),
        });
    }
    out
}

pub fn default_model(config: &LoadedConfig) -> Option<ModelInfo> {
    let all = models(config);
    if let Some(reference) = config.config.model.as_deref() {
        if let Some((provider, id, _variant)) = config.parse_model(reference) {
            if let Some(model) = all
                .iter()
                .find(|model| model.provider_id == provider && model.model_id == id)
            {
                return Some(model.clone());
            }
        }
    }
    all.iter()
        .find(|model| model.enabled)
        .cloned()
        .or_else(|| all.first().cloned())
}

pub fn resolve(config: &LoadedConfig, reference: &ModelRef) -> Option<ResolvedModel> {
    let catalog = catalog();
    let provider = config.provider(&reference.provider_id);
    let raw_provider = catalog.providers.get(&reference.provider_id);

    let protocol = match reference.provider_id.as_str() {
        "anthropic" => Protocol::Anthropic,
        "mock" => Protocol::Mock,
        "google" => Protocol::OpenAi, // Gemini's OpenAI-compatible endpoint
        _ => Protocol::OpenAi,
    };

    let api = raw_provider.and_then(|provider| provider.api.as_deref());
    let base_url = if reference.provider_id == "mock" {
        "mock://local".to_string()
    } else {
        base_url(config, &reference.provider_id, api)
    };
    let api_key = provider
        .and_then(|provider| provider.api_key().map(str::to_string))
        .or_else(|| {
            raw_provider
                .map(|provider| provider.env.clone())
                .and_then(|env| env_key(&reference.provider_id, &env))
        });
    let headers = provider
        .map(|provider| provider.headers().clone())
        .unwrap_or_default();
    let package = provider
        .and_then(|provider| provider.npm.clone())
        .or_else(|| raw_provider.and_then(|provider| provider.npm.clone()))
        .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string());

    Some(ResolvedModel {
        provider_id: reference.provider_id.clone(),
        model_id: reference.id.clone(),
        variant: reference.variant.clone(),
        base_url,
        api_key,
        headers,
        protocol,
        package,
    })
}

pub fn resolve_by_string(config: &LoadedConfig, value: &str) -> Option<ResolvedModel> {
    let (provider, rest) = value.split_once('/')?;
    let (id, variant) = match rest.split_once('#') {
        Some((id, variant)) if !variant.is_empty() => (id.to_string(), Some(variant.to_string())),
        _ => (rest.to_string(), None),
    };
    resolve(
        config,
        &ModelRef {
            id,
            provider_id: provider.to_string(),
            variant,
        },
    )
}
