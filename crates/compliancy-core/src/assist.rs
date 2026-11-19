//! Assist-mode optional LLM provider (OpenAI-compatible HTTP).
//!
//! Standard mode never constructs a live client and never makes network calls.
//! Assist mode may call a provider only for ISO gap narrative and freeform
//! context interpretation when configured (or mock fixtures for CI).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Environment / local-config keys (never commit secrets).
pub const ENV_PROVIDER: &str = "COMPLIANCY_ASSIST_PROVIDER";
pub const ENV_API_KEY: &str = "COMPLIANCY_ASSIST_API_KEY";
pub const ENV_ENDPOINT: &str = "COMPLIANCY_ASSIST_ENDPOINT";
pub const ENV_MODEL: &str = "COMPLIANCY_ASSIST_MODEL";
pub const ENV_CONFIG_PATH: &str = "COMPLIANCY_ASSIST_CONFIG";
pub const ENV_MOCK: &str = "COMPLIANCY_ASSIST_MOCK";

const DEFAULT_OPENAI_ENDPOINT: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistConfig {
    /// Provider id: `none` | `openai` | `azure` | `local` | `mock`.
    pub provider: String,
    /// API key — UI session, env, or local config file; never logged.
    pub api_key: Option<String>,
    /// Optional custom endpoint (OpenAI-compatible base URL, or full Azure chat URL).
    pub endpoint: Option<String>,
    /// Optional model / deployment name.
    pub model: Option<String>,
}

impl Default for AssistConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            api_key: None,
            endpoint: None,
            model: None,
        }
    }
}

impl AssistConfig {
    /// Merge UI/session config with env vars and optional local JSON config.
    /// Precedence (highest last): file → env → UI fields that are non-empty.
    pub fn resolve(ui: Option<AssistConfig>) -> Self {
        let mut cfg = Self::default();

        if let Some(path) = std::env::var_os(ENV_CONFIG_PATH) {
            if let Ok(file_cfg) = load_assist_config_file(Path::new(&path)) {
                cfg.merge_from(&file_cfg);
            }
        } else if let Some(default_path) = default_assist_config_path() {
            if default_path.is_file() {
                if let Ok(file_cfg) = load_assist_config_file(&default_path) {
                    cfg.merge_from(&file_cfg);
                }
            }
        }

        cfg.merge_from(&AssistConfig {
            provider: std::env::var(ENV_PROVIDER).unwrap_or_default(),
            api_key: std::env::var(ENV_API_KEY).ok().filter(|s| !s.is_empty()),
            endpoint: std::env::var(ENV_ENDPOINT).ok().filter(|s| !s.is_empty()),
            model: std::env::var(ENV_MODEL).ok().filter(|s| !s.is_empty()),
        });

        if let Some(ui) = ui {
            cfg.merge_from(&ui);
        }

        if env_truthy(ENV_MOCK) && (cfg.provider.is_empty() || cfg.provider == "none") {
            cfg.provider = "mock".into();
        }

        if cfg.provider.is_empty() {
            cfg.provider = "none".into();
        }

        cfg
    }

    fn merge_from(&mut self, other: &AssistConfig) {
        if !other.provider.trim().is_empty() {
            self.provider = other.provider.trim().to_string();
        }
        if let Some(key) = other.api_key.as_ref().filter(|k| !k.is_empty()) {
            self.api_key = Some(key.clone());
        }
        if let Some(ep) = other.endpoint.as_ref().filter(|e| !e.is_empty()) {
            self.endpoint = Some(ep.clone());
        }
        if let Some(m) = other.model.as_ref().filter(|m| !m.is_empty()) {
            self.model = Some(m.clone());
        }
    }

    pub fn is_configured(&self) -> bool {
        let p = self.provider.trim().to_ascii_lowercase();
        if p.is_empty() || p == "none" {
            return false;
        }
        if p == "mock" || env_truthy(ENV_MOCK) {
            return true;
        }
        self.api_key
            .as_ref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn wants_mock(&self) -> bool {
        self.provider.eq_ignore_ascii_case("mock") || env_truthy(ENV_MOCK)
    }

    pub fn resolved_model(&self) -> String {
        self.model
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }

    pub fn resolved_endpoint(&self) -> String {
        self.endpoint
            .as_ref()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_OPENAI_ENDPOINT.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistStatus {
    pub mode: String,
    pub provider: String,
    pub configured: bool,
    pub live_calls_enabled: bool,
    pub message: String,
}

/// Trait for Assist LLM backends.
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn complete(&self, system: &str, user: &str) -> Result<String, String>;
}

/// Offline placeholder — never contacts the network.
pub struct OfflineProvider {
    reason: String,
}

impl OfflineProvider {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl LlmProvider for OfflineProvider {
    fn name(&self) -> &str {
        "offline"
    }

    fn complete(&self, _system: &str, _user: &str) -> Result<String, String> {
        Err(self.reason.clone())
    }
}

/// Deterministic fixture provider for CI / local dry-runs (no network).
pub struct MockLlmProvider;

impl LlmProvider for MockLlmProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        let kind = if system.to_ascii_lowercase().contains("iso")
            || user.to_ascii_lowercase().contains("checklist")
            || user.to_ascii_lowercase().contains("gap")
        {
            "iso_gap"
        } else {
            "freeform_context"
        };

        Ok(match kind {
            "iso_gap" => {
                "MOCK ISO gap narrative\n\
                 - Review missing clauses listed in the checklist export.\n\
                 - Add or expand sections that cover each MISS row.\n\
                 - This is assistive guidance only — not an accreditation decision.\n\
                 [fixture]"
                    .into()
            }
            _ => {
                "MOCK freeform context interpretation\n\
                 Suggested structured rules:\n\
                 force_heading: Document Control\n\
                 require_section: Scope\n\
                 replace: DRAFT => FINAL\n\
                 Notes: Prefer the deterministic rule language in Standard mode when possible.\n\
                 [fixture]"
                    .into()
            }
        })
    }
}

/// OpenAI-compatible chat-completions client (OpenAI, Azure, local OpenAI-like servers).
pub struct OpenAiCompatibleProvider {
    pub provider_id: String,
    pub api_key: String,
    pub endpoint: String,
    pub model: String,
    /// Injected for tests; defaults to live ureq POST.
    pub http: Box<dyn ChatHttp + Send + Sync>,
}

pub trait ChatHttp: Send + Sync {
    fn post_json(&self, url: &str, api_key: &str, body: &Value) -> Result<Value, String>;
}

pub struct UreqChatHttp;

impl ChatHttp for UreqChatHttp {
    fn post_json(&self, url: &str, api_key: &str, body: &Value) -> Result<Value, String> {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(60))
            .build();
        let response = agent
            .post(url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json")
            .set("api-key", api_key) // Azure OpenAI also accepts api-key
            .send_json(body.clone())
            .map_err(|e| format!("Assist HTTP request failed: {e}"))?;

        let status = response.status();
        let parsed: Value = response
            .into_json()
            .map_err(|e| format!("Assist response was not JSON: {e}"))?;

        if !(200..300).contains(&status) {
            let msg = parsed
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Assist API HTTP {status}: {msg}"));
        }
        Ok(parsed)
    }
}

impl OpenAiCompatibleProvider {
    pub fn from_config(config: &AssistConfig) -> Result<Self, String> {
        let key = config
            .api_key
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Assist API key missing. Set COMPLIANCY_ASSIST_API_KEY, a local config file, or the Assist UI field."
                    .to_string()
            })?;

        Ok(Self {
            provider_id: config.provider.clone(),
            api_key: key,
            endpoint: config.resolved_endpoint(),
            model: config.resolved_model(),
            http: Box::new(UreqChatHttp),
        })
    }

    fn chat_url(&self) -> String {
        let base = self.endpoint.trim_end_matches('/');
        // Azure often uses a full deployment URL ending in /chat/completions.
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.provider_id
    }

    fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });

        let parsed = self.http.post_json(&self.chat_url(), &self.api_key, &body)?;
        extract_chat_content(&parsed)
    }
}

fn extract_chat_content(parsed: &Value) -> Result<String, String> {
    parsed
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Assist API returned empty chat content".into())
}

/// Resolved Assist runtime for a pipeline run.
pub struct AssistRuntime {
    pub status: AssistStatus,
    pub provider: Box<dyn LlmProvider>,
}

impl AssistRuntime {
    /// Build provider for `standard` or `assist` mode.
    /// Standard always gets an offline provider (no HTTP client constructed).
    pub fn for_mode(mode: &str, ui_config: Option<AssistConfig>) -> Self {
        let config = AssistConfig::resolve(ui_config);
        let configured = config.is_configured();

        if mode != "assist" {
            return Self {
                status: AssistStatus {
                    mode: mode.into(),
                    provider: config.provider.clone(),
                    configured: false,
                    live_calls_enabled: false,
                    message: "Standard mode: Assist provider is unused (offline).".into(),
                },
                provider: Box::new(OfflineProvider::new(
                    "Standard mode does not call Assist providers.",
                )),
            };
        }

        if !configured {
            return Self {
                status: AssistStatus {
                    mode: "assist".into(),
                    provider: config.provider.clone(),
                    configured: false,
                    live_calls_enabled: false,
                    message: "Assist mode active, but provider/API key is not configured. Semantic steps are skipped.".into(),
                },
                provider: Box::new(OfflineProvider::new(
                    "Assist not configured: set provider + API key (env, local config, or UI).",
                )),
            };
        }

        if config.wants_mock() {
            return Self {
                status: AssistStatus {
                    mode: "assist".into(),
                    provider: "mock".into(),
                    configured: true,
                    live_calls_enabled: false,
                    message: "Assist mock provider active (fixture responses; no network).".into(),
                },
                provider: Box::new(MockLlmProvider),
            };
        }

        match OpenAiCompatibleProvider::from_config(&config) {
            Ok(client) => Self {
                status: AssistStatus {
                    mode: "assist".into(),
                    provider: config.provider.clone(),
                    configured: true,
                    live_calls_enabled: true,
                    message: format!(
                        "Assist live API ready (provider '{}', model '{}').",
                        config.provider,
                        config.resolved_model()
                    ),
                },
                provider: Box::new(client),
            },
            Err(e) => Self {
                status: AssistStatus {
                    mode: "assist".into(),
                    provider: config.provider.clone(),
                    configured: false,
                    live_calls_enabled: false,
                    message: e.clone(),
                },
                provider: Box::new(OfflineProvider::new(e)),
            },
        }
    }

    pub fn can_call(&self) -> bool {
        self.status.configured
            && (self.status.live_calls_enabled || self.status.provider == "mock")
    }
}

const ISO_SYSTEM: &str = "You are an ISO management-system documentation assistant for Compliancy Checker. \
Given checklist findings (presence/missing only), write a concise gap narrative for a human reviewer. \
Do not claim accreditation. Prefer actionable section/heading suggestions. Keep under 400 words.";

const CONTEXT_SYSTEM: &str = "You are a documentation rules assistant for Compliancy Checker. \
The product applies a deterministic rule language (force_heading, require_section, require_heading, replace, document:). \
When freeform instructions cannot be fully expressed as those ops, interpret them: summarize intent, \
suggest equivalent structured rules when possible, and note what still needs human judgment. \
Do not invent secret content. Keep under 400 words.";

/// Prompt + call for ISO gap narrative over checklist text.
pub fn iso_gap_narrative(provider: &dyn LlmProvider, checklist_report: &str) -> Result<String, String> {
    let user = format!(
        "Write an ISO gap narrative from these checklist findings:\n\n{checklist_report}"
    );
    provider.complete(ISO_SYSTEM, &user)
}

/// Prompt + call for freeform context interpretation.
pub fn interpret_freeform_context(
    provider: &dyn LlmProvider,
    freeform: &str,
    structured_summary: Option<&str>,
) -> Result<String, String> {
    let mut user = String::from("Interpret this freeform context for document processing:\n\n");
    user.push_str(freeform.trim());
    user.push('\n');
    if let Some(summary) = structured_summary {
        user.push_str("\nStructured rules already applied or parsed:\n");
        user.push_str(summary);
        user.push('\n');
    }
    provider.complete(CONTEXT_SYSTEM, &user)
}

/// True when context text looks like freeform prose beyond (or instead of) the rule language.
pub fn looks_like_freeform_context(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // JSON rule docs are structured.
    if trimmed.starts_with('{') {
        return false;
    }

    let mut substantive = 0usize;
    let mut rule_like = 0usize;
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        substantive += 1;
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("force_heading:")
            || lower.starts_with("force_section:")
            || lower.starts_with("require_section:")
            || lower.starts_with("require_heading:")
            || lower.starts_with("replace:")
            || lower.starts_with("document:")
        {
            rule_like += 1;
        }
    }

    if substantive == 0 {
        return false;
    }
    // Any non-rule substantive line ⇒ freeform worth Assist interpretation.
    rule_like < substantive
}

pub fn write_text_export(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, contents).map_err(|e| e.to_string())
}

fn load_assist_config_file(path: &Path) -> Result<AssistConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut cfg: AssistConfig = serde_json::from_str(&raw)
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    // Also accept snake_case keys via a loose Value merge.
    if let Ok(v) = serde_json::from_str::<Value>(&raw) {
        if cfg.api_key.is_none() {
            cfg.api_key = v
                .get("api_key")
                .or_else(|| v.get("apiKey"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
        }
    }
    Ok(cfg)
}

fn default_assist_config_path() -> Option<PathBuf> {
    // Prefer XDG / home local config; never required.
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home)
            .join(".config")
            .join("compliancy-checker")
            .join("assist.json");
        return Some(p);
    }
    None
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref().map(|s| s.trim().to_ascii_lowercase()),
        Some(ref s) if s == "1" || s == "true" || s == "yes" || s == "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct CaptureHttp {
        last_url: Mutex<String>,
        last_body: Mutex<Value>,
        response: Value,
    }

    impl ChatHttp for CaptureHttp {
        fn post_json(&self, url: &str, _api_key: &str, body: &Value) -> Result<Value, String> {
            *self.last_url.lock().unwrap() = url.to_string();
            *self.last_body.lock().unwrap() = body.clone();
            Ok(self.response.clone())
        }
    }

    #[test]
    fn standard_mode_never_enables_live_calls() {
        let _g = env_lock().lock().unwrap();
        std::env::remove_var(ENV_MOCK);
        std::env::remove_var(ENV_API_KEY);
        std::env::remove_var(ENV_PROVIDER);

        let rt = AssistRuntime::for_mode(
            "standard",
            Some(AssistConfig {
                provider: "openai".into(),
                api_key: Some("sk-test".into()),
                endpoint: None,
                model: Some("gpt-test".into()),
            }),
        );
        assert!(!rt.status.live_calls_enabled);
        assert!(!rt.can_call());
        assert!(rt.provider.complete("sys", "user").is_err());
    }

    #[test]
    fn mock_provider_returns_fixture_without_network() {
        let _g = env_lock().lock().unwrap();
        std::env::remove_var(ENV_API_KEY);
        std::env::set_var(ENV_MOCK, "1");

        let rt = AssistRuntime::for_mode(
            "assist",
            Some(AssistConfig {
                provider: "none".into(),
                api_key: None,
                endpoint: None,
                model: None,
            }),
        );
        assert!(rt.can_call());
        assert_eq!(rt.status.provider, "mock");
        assert!(!rt.status.live_calls_enabled);

        let iso = iso_gap_narrative(rt.provider.as_ref(), "MISS 4.1 Context").unwrap();
        assert!(iso.contains("MOCK ISO gap narrative"));
        assert!(iso.contains("[fixture]"));

        let ctx = interpret_freeform_context(
            rt.provider.as_ref(),
            "Please make every third heading bold and fix typos",
            None,
        )
        .unwrap();
        assert!(ctx.contains("MOCK freeform context"));

        std::env::remove_var(ENV_MOCK);
    }

    #[test]
    fn openai_compatible_parses_chat_content_and_request_shape() {
        use std::sync::Arc;

        struct SharedHttp {
            last_url: Mutex<String>,
            last_body: Mutex<Value>,
            response: Value,
        }

        impl ChatHttp for SharedHttp {
            fn post_json(&self, url: &str, _api_key: &str, body: &Value) -> Result<Value, String> {
                *self.last_url.lock().unwrap() = url.to_string();
                *self.last_body.lock().unwrap() = body.clone();
                Ok(self.response.clone())
            }
        }

        let http = Arc::new(SharedHttp {
            last_url: Mutex::new(String::new()),
            last_body: Mutex::new(json!({})),
            response: json!({
                "choices": [{ "message": { "content": "  Hello from model  " } }]
            }),
        });

        struct ArcHttp(Arc<SharedHttp>);
        impl ChatHttp for ArcHttp {
            fn post_json(&self, url: &str, api_key: &str, body: &Value) -> Result<Value, String> {
                self.0.post_json(url, api_key, body)
            }
        }

        let provider = OpenAiCompatibleProvider {
            provider_id: "openai".into(),
            api_key: "sk-test".into(),
            endpoint: "https://example.test/v1".into(),
            model: "gpt-test".into(),
            http: Box::new(ArcHttp(Arc::clone(&http))),
        };
        let out = provider.complete("system", "user prompt").unwrap();
        assert_eq!(out, "Hello from model");
        assert_eq!(
            http.last_url.lock().unwrap().as_str(),
            "https://example.test/v1/chat/completions"
        );
        let body = http.last_body.lock().unwrap().clone();
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "user prompt");
    }

    #[test]
    fn chat_url_helpers_and_extract() {
        let p = OpenAiCompatibleProvider {
            provider_id: "openai".into(),
            api_key: "k".into(),
            endpoint: "https://api.openai.com/v1/".into(),
            model: "m".into(),
            http: Box::new(CaptureHttp {
                last_url: Mutex::new(String::new()),
                last_body: Mutex::new(json!({})),
                response: json!({}),
            }),
        };
        assert_eq!(
            p.chat_url(),
            "https://api.openai.com/v1/chat/completions"
        );

        let azure = OpenAiCompatibleProvider {
            provider_id: "azure".into(),
            api_key: "k".into(),
            endpoint: "https://ex.openai.azure.com/openai/deployments/d/chat/completions?api-version=1".into(),
            model: "d".into(),
            http: Box::new(CaptureHttp {
                last_url: Mutex::new(String::new()),
                last_body: Mutex::new(json!({})),
                response: json!({}),
            }),
        };
        assert!(azure.chat_url().contains("/chat/completions?api-version=1"));
        assert!(!azure.chat_url().contains("/chat/completions/chat/completions"));

        assert!(extract_chat_content(&json!({})).is_err());
        assert_eq!(
            extract_chat_content(&json!({"choices":[{"message":{"content":"x"}}]})).unwrap(),
            "x"
        );
    }

    #[test]
    fn freeform_detection() {
        assert!(!looks_like_freeform_context("force_heading: Scope\nreplace: a => b"));
        assert!(looks_like_freeform_context(
            "Please rewrite the tone to be more formal and ensure OH&S risks are covered."
        ));
        assert!(looks_like_freeform_context(
            "force_heading: Scope\nAlso make the language more formal overall."
        ));
        assert!(!looks_like_freeform_context(r#"{"rules":[]}"#));
    }

    #[test]
    fn resolve_prefers_ui_over_env() {
        let _g = env_lock().lock().unwrap();
        std::env::set_var(ENV_PROVIDER, "local");
        std::env::set_var(ENV_MODEL, "env-model");
        std::env::remove_var(ENV_MOCK);

        let cfg = AssistConfig::resolve(Some(AssistConfig {
            provider: "openai".into(),
            api_key: Some("ui-key".into()),
            endpoint: Some("https://ui.example/v1".into()),
            model: Some("ui-model".into()),
        }));
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.api_key.as_deref(), Some("ui-key"));
        assert_eq!(cfg.model.as_deref(), Some("ui-model"));
        assert_eq!(cfg.endpoint.as_deref(), Some("https://ui.example/v1"));

        std::env::remove_var(ENV_PROVIDER);
        std::env::remove_var(ENV_MODEL);
    }
}
