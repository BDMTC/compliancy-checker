//! Assist-mode LLM provider hook (stubbed).
//!
//! Standard mode never constructs or calls a provider. Assist mode may load
//! config and obtain a stub that refuses live network calls until wired.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistConfig {
    /// Provider id: `none` | `openai` | `azure` | `local` (extensible).
    pub provider: String,
    /// API key — stored only in memory for this spike; never logged.
    pub api_key: Option<String>,
    /// Optional custom endpoint / Azure resource URL.
    pub endpoint: Option<String>,
    /// Optional model name.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistStatus {
    pub mode: String,
    pub provider: String,
    pub configured: bool,
    pub live_calls_enabled: bool,
    pub message: String,
}

/// Trait for future real providers. Spike ships only [`StubLlmProvider`].
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn complete(&self, prompt: &str) -> Result<String, String>;
}

/// No-op / refuse-to-call provider used until a real HTTP client is added.
pub struct StubLlmProvider {
    pub config: AssistConfig,
}

impl StubLlmProvider {
    pub fn from_config(config: AssistConfig) -> Self {
        Self { config }
    }

    pub fn status(&self, app_mode: &str) -> AssistStatus {
        let configured = self.config.provider != "none"
            && self
                .config
                .api_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false);

        let message = if app_mode != "assist" {
            "Standard mode: Assist provider is unused (offline).".into()
        } else if !configured {
            "Assist mode active, but provider/API key is not configured. Semantic steps are skipped."
                .into()
        } else {
            format!(
                "Assist configured for provider '{}', but live API calls are stubbed in this spike.",
                self.config.provider
            )
        };

        AssistStatus {
            mode: app_mode.into(),
            provider: self.config.provider.clone(),
            configured,
            live_calls_enabled: false,
            message,
        }
    }
}

impl LlmProvider for StubLlmProvider {
    fn name(&self) -> &str {
        "stub"
    }

    fn complete(&self, _prompt: &str) -> Result<String, String> {
        Err(
            "Assist LLM hook is stubbed: no live network calls in this spike. Configure provider later."
                .into(),
        )
    }
}
