//! Atlas Cloud provider, mirroring `windows_use/providers/atlas_cloud/llm.py`.
//!
//! Atlas Cloud exposes an OpenAI-compatible endpoint at
//! `https://api.atlascloud.ai/v1`.

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for Atlas Cloud.
pub const ATLAS_CLOUD_BASE_URL: &str = "https://api.atlascloud.ai/v1";
/// Environment variable holding the Atlas Cloud API key.
pub const ATLAS_CLOUD_API_KEY_ENV: &str = "ATLAS_CLOUD_API_KEY";
/// Environment variable overriding the Atlas Cloud base URL.
pub const ATLAS_CLOUD_API_BASE_ENV: &str = "ATLAS_CLOUD_API_BASE";
/// Upstream default model.
pub const ATLAS_CLOUD_DEFAULT_MODEL: &str = "openai/gpt-4.1-mini";

/// The configuration of the Atlas Cloud provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "atlas_cloud",
        completions_path: "/chat/completions",
        default_base_url: ATLAS_CLOUD_BASE_URL,
        base_url_env: &[ATLAS_CLOUD_API_BASE_ENV],
        api_key_env: &[ATLAS_CLOUD_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            reasoning_as_reasoning_content: true,
            accept_reasoning_field: true,
            reasoning_model_rule: ReasoningModelRule::Never,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::Fixed,
        fixed_context_window: 128_000,
        sends_reasoning_effort: false,
    }
}

openai_compatible_newtype!(
    /// Atlas Cloud chat model, mirroring `ChatAtlasCloud`.
    ChatAtlasCloud
);

impl ChatAtlasCloud {
    /// Mirrors `ChatAtlasCloud.__init__` (delegating to `ChatOpenAI`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        kwargs: Map<String, Value>,
    ) -> Self {
        Self {
            inner: OpenAiCompatible::new(
                config(),
                model,
                api_key,
                base_url,
                timeout,
                max_retries,
                None,
                temperature,
                None,
                kwargs,
            ),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, 600.0, 2, None, Map::new())
    }
}
