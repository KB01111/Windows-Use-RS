//! Cerebras provider, mirroring `windows_use/providers/cerebras/llm.py`.
//!
//! Cerebras asks for `reasoning_format=parsed`, adds `reasoning_effort` for
//! GPT-OSS, and embeds thinking directly in the assistant text: GPT-OSS gets a
//! plain prefix, GLM/Qwen get `<think>` tags.

use serde_json::{Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for the Cerebras API.
pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
/// Environment variable holding the Cerebras API key.
pub const CEREBRAS_API_KEY_ENV: &str = "CEREBRAS_API_KEY";
/// Upstream default model.
pub const CEREBRAS_DEFAULT_MODEL: &str = "llama-3.3-70b";

/// The configuration of the Cerebras provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "cerebras",
        completions_path: "/chat/completions",
        default_base_url: CEREBRAS_BASE_URL,
        base_url_env: &[],
        api_key_env: &[CEREBRAS_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            inline_thinking: true,
            reasoning_as_reasoning_content: false,
            accept_reasoning_field: true,
            reasoning_model_rule: ReasoningModelRule::Contains,
            ..CompatQuirks::default()
        },
        reasoning_needles: &["gpt-oss", "zai-glm", "qwen3"],
        context_window: ContextWindowRule::Fixed,
        fixed_context_window: 8_192,
        sends_reasoning_effort: false,
    }
}

openai_compatible_newtype!(
    /// Cerebras chat model, mirroring `ChatCerebras`.
    ChatCerebras
);

impl ChatCerebras {
    /// Mirrors `ChatCerebras.__init__` (default timeout `60.0`, 2 retries).
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

    /// Mirrors `_format_assistant_content`: how Cerebras replays thinking.
    pub fn format_content(&self, content: Option<&str>, thinking: Option<&str>) -> String {
        crate::providers::openai::format_assistant_content(&self.inner.model, content, thinking)
    }
}
