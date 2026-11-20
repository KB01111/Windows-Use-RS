//! DeepSeek provider, mirroring `windows_use/providers/deepseek/llm.py`.
//!
//! DeepSeek is OpenAI-compatible at `https://api.deepseek.com/v1` and enables its
//! reasoning mode through a `thinking` body flag; `deepseek-reasoner` models are
//! treated as reasoning models so `temperature` is dropped.

use serde_json::{json, Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for the DeepSeek API.
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
/// Environment variable holding the DeepSeek API key.
pub const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
/// Environment variable overriding the DeepSeek base URL.
pub const DEEPSEEK_API_BASE_ENV: &str = "DEEPSEEK_API_BASE";
/// Upstream default model.
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-chat";

/// The configuration of the DeepSeek provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "deepseek",
        completions_path: "/chat/completions",
        default_base_url: DEEPSEEK_BASE_URL,
        base_url_env: &[DEEPSEEK_API_BASE_ENV],
        api_key_env: &[DEEPSEEK_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            reasoning_as_reasoning_content: true,
            accept_reasoning_field: true,
            reasoning_model_rule: ReasoningModelRule::ReasonerSubstring,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::Fixed,
        fixed_context_window: 128_000,
        sends_reasoning_effort: true,
    }
}

openai_compatible_newtype!(
    /// DeepSeek chat model, mirroring `ChatDeepSeek`.
    ChatDeepSeek
);

impl ChatDeepSeek {
    /// Mirrors `ChatDeepSeek.__init__`.
    ///
    /// `thinking` toggles DeepSeek's reasoning mode through
    /// `extra_body={"thinking": {"type": "enabled" | "disabled"}}`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        reasoning_effort: Option<String>,
        thinking: bool,
        kwargs: Map<String, Value>,
    ) -> Self {
        let extra_body = json!({
            "thinking": {"type": if thinking { "enabled" } else { "disabled" }},
        });

        Self {
            inner: OpenAiCompatible::new(
                config(),
                model,
                api_key,
                base_url,
                timeout,
                max_retries,
                Some(reasoning_effort.unwrap_or_else(|| "high".to_string())),
                temperature,
                Some(extra_body),
                kwargs,
            ),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, 600.0, 2, None, None, false, Map::new())
    }

    /// The `thinking` flag currently configured on the request body.
    pub fn thinking_flag(&self) -> bool {
        self.inner
            .extra_body
            .as_ref()
            .and_then(|body| body.get("thinking"))
            .and_then(|thinking| thinking.get("type"))
            .and_then(Value::as_str)
            .map(|kind| kind == "enabled")
            .unwrap_or(false)
    }
}
