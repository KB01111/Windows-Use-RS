//! OpenRouter provider, mirroring `windows_use/providers/open_router/llm.py`.
//!
//! OpenRouter is OpenAI-compatible at `https://openrouter.ai/api/v1`; it does not
//! support the structured-output beta endpoint, so `json_mode` is used instead.

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for OpenRouter.
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Environment variable holding the OpenRouter API key.
pub const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// The configuration of the OpenRouter provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "open_router",
        completions_path: "/chat/completions",
        default_base_url: OPENROUTER_BASE_URL,
        base_url_env: &[],
        api_key_env: &[OPENROUTER_API_KEY_ENV],
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
    /// OpenRouter chat model, mirroring `ChatOpenRouter`.
    ChatOpenRouter
);

impl ChatOpenRouter {
    /// Mirrors `ChatOpenRouter.__init__`. `model` is required in Python.
    ///
    /// `default_headers` become extra request headers (e.g. `HTTP-Referer`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        default_headers: Option<Map<String, Value>>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let mut config = config();
        if let Some(headers) = &default_headers {
            let extra: Vec<(String, String)> = headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_string()))
                })
                .collect();
            if !extra.is_empty() {
                config.auth = AuthStyle::BearerWithHeaders(extra);
            }
        }

        Self {
            inner: OpenAiCompatible::new(
                config,
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
        Self::new(model, None, None, 600.0, 2, None, None, Map::new())
    }
}
