//! Groq provider, mirroring `windows_use/providers/groq/llm.py`.
//!
//! Groq's API is OpenAI-compatible but serializes reasoning as `reasoning`, asks
//! for `reasoning_format=parsed` on Qwen models and `include_reasoning` elsewhere,
//! and never accepts `temperature` on its reasoning models.

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for the Groq API.
pub const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
/// Environment variable holding the Groq API key.
pub const GROQ_API_KEY_ENV: &str = "GROQ_API_KEY";
/// Upstream default model.
pub const GROQ_DEFAULT_MODEL: &str = "llama-3.3-70b-versatile";

/// The configuration of the Groq provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "groq",
        completions_path: "/chat/completions",
        default_base_url: GROQ_BASE_URL,
        base_url_env: &[],
        api_key_env: &[GROQ_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            // Groq uses `reasoning` rather than `reasoning_content`.
            reasoning_as_reasoning_content: false,
            accept_reasoning_field: true,
            groq_reasoning_params: true,
            reasoning_model_rule: ReasoningModelRule::Contains,
            ..CompatQuirks::default()
        },
        reasoning_needles: &["gpt-oss", "qwen3-32b", "qwen/qwen3"],
        context_window: ContextWindowRule::GroqFamily,
        fixed_context_window: 128_000,
        sends_reasoning_effort: false,
    }
}

openai_compatible_newtype!(
    /// Groq chat model, mirroring `ChatGroq`.
    ChatGroq
);

impl ChatGroq {
    /// Mirrors `ChatGroq.__init__` (default timeout `60.0`, 2 retries).
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
        Self::new(model, None, None, 60.0, 2, None, Map::new())
    }
}
