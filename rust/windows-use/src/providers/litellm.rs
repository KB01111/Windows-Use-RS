//! LiteLLM-compatible provider, mirroring `windows_use/providers/litellm/llm.py`.
//!
//! The Python class delegates to the `litellm` SDK, which multiplexes 100+
//! providers behind the OpenAI wire format. This port speaks that same wire
//! format directly: the model string keeps its `provider/model` form and the
//! caller supplies the matching gateway endpoint (`base_url`).

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, parse_usage, AuthStyle, CompatConfig, CompatQuirks,
    ContextWindowRule, OpenAiCompatible, ReasoningModelRule,
};
use crate::providers::views::TokenUsage;

/// Environment variable consulted when no API key is passed.
pub const LITELLM_API_KEY_ENV: &str = "LITELLM_API_KEY";

/// The configuration of the LiteLLM provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "litellm",
        completions_path: "/chat/completions",
        // LiteLLM proxies are always self-hosted, so there is no sane default.
        default_base_url: "http://localhost:4000",
        base_url_env: &["LITELLM_BASE_URL", "LITELLM_PROXY_BASE_URL"],
        api_key_env: &[LITELLM_API_KEY_ENV],
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
    /// LiteLLM gateway chat model, mirroring `ChatLiteLLM`.
    ChatLiteLLM
);

impl ChatLiteLLM {
    /// Mirrors `ChatLiteLLM.__init__`.
    ///
    /// `drop_params` is accepted for API parity; dropping unsupported params is
    /// the gateway's job when calling through HTTP.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        drop_params: bool,
        kwargs: Map<String, Value>,
    ) -> Self {
        let mut kwargs = kwargs;
        kwargs.insert("drop_params".to_string(), Value::Bool(drop_params));

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
        Self::new(model, None, None, 600.0, 2, None, true, Map::new())
    }

    /// `_extract_usage`: LiteLLM surfaces cache counters too.
    pub fn extract_usage(usage: Option<&Value>) -> TokenUsage {
        parse_usage(usage)
    }
}
