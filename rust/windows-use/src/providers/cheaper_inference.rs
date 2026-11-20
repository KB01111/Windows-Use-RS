//! Cheaper Inference provider, mirroring
//! `windows_use/providers/cheaper_inference/llm.py`.
//!
//! Cheaper Inference exposes an OpenAI-compatible endpoint at
//! `https://api.cheaperinference.com/v1`.

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for Cheaper Inference.
pub const CHEAPER_INFERENCE_BASE_URL: &str = "https://api.cheaperinference.com/v1";
/// Environment variable holding the Cheaper Inference API key.
pub const CHEAPER_INFERENCE_API_KEY_ENV: &str = "CHEAPER_INFERENCE_API_KEY";
/// Environment variable overriding the Cheaper Inference base URL.
pub const CHEAPER_INFERENCE_API_BASE_ENV: &str = "CHEAPER_INFERENCE_API_BASE";
/// Upstream default model.
pub const CHEAPER_INFERENCE_DEFAULT_MODEL: &str = "gpt-5.4-mini";

/// The configuration of the Cheaper Inference provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "cheaper_inference",
        completions_path: "/chat/completions",
        default_base_url: CHEAPER_INFERENCE_BASE_URL,
        base_url_env: &[CHEAPER_INFERENCE_API_BASE_ENV],
        api_key_env: &[CHEAPER_INFERENCE_API_KEY_ENV],
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
    /// Cheaper Inference chat model, mirroring `ChatCheaperInference`.
    ChatCheaperInference
);

impl ChatCheaperInference {
    /// Mirrors `ChatCheaperInference.__init__` (delegating to `ChatOpenAI`).
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
