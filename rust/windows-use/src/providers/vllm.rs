//! vLLM provider, mirroring `windows_use/providers/vllm/llm.py`.
//!
//! vLLM serves an OpenAI-compatible endpoint (default
//! `http://localhost:8000/v1`) and takes no real API key (`EMPTY` by default).
//! Thinking is enabled through `chat_template_kwargs={"enable_thinking": ...}`.

use serde_json::{json, Map, Value};

use crate::providers::base::resolve_setting;
use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for a local vLLM server.
pub const VLLM_BASE_URL: &str = "http://localhost:8000/v1";
/// Environment variable overriding the vLLM base URL.
pub const VLLM_BASE_URL_ENV: &str = "VLLM_BASE_URL";
/// Environment variable holding the vLLM API key.
pub const VLLM_API_KEY_ENV: &str = "VLLM_API_KEY";
/// Placeholder key used when the server requires none.
pub const VLLM_EMPTY_API_KEY: &str = "EMPTY";
/// Default context window assumed when the server reports none.
pub const VLLM_DEFAULT_CONTEXT_WINDOW: i64 = 32_768;

/// The configuration of the vLLM provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "vllm",
        completions_path: "/chat/completions",
        default_base_url: VLLM_BASE_URL,
        base_url_env: &[VLLM_BASE_URL_ENV],
        api_key_env: &[VLLM_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            reasoning_as_reasoning_content: true,
            accept_reasoning_field: true,
            reasoning_model_rule: ReasoningModelRule::Never,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::Fixed,
        fixed_context_window: VLLM_DEFAULT_CONTEXT_WINDOW,
        sends_reasoning_effort: false,
    }
}

openai_compatible_newtype!(
    /// vLLM chat model, mirroring `ChatVLLM`.
    ChatVLLM
);

impl ChatVLLM {
    /// Mirrors `ChatVLLM.__init__`.
    ///
    /// The API key defaults to `EMPTY` (vLLM requires none) and the base URL to
    /// `http://localhost:8000/v1`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        thinking: bool,
        kwargs: Map<String, Value>,
    ) -> Self {
        let api_key = Some(
            resolve_setting(api_key, &[VLLM_API_KEY_ENV])
                .unwrap_or_else(|| VLLM_EMPTY_API_KEY.to_string()),
        );
        let extra_body = json!({"chat_template_kwargs": {"enable_thinking": thinking}});

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
                Some(extra_body),
                kwargs,
            ),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, 600.0, 2, None, false, Map::new())
    }

    /// The configured `enable_thinking` flag.
    pub fn thinking_flag(&self) -> bool {
        self.inner
            .extra_body
            .as_ref()
            .and_then(|body| body.get("chat_template_kwargs"))
            .and_then(|kwargs| kwargs.get("enable_thinking"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}
