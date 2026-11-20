//! NVIDIA NIM provider, mirroring `windows_use/providers/nvidia/llm.py`.
//!
//! NVIDIA's NIM API is OpenAI-compatible at
//! `https://integrate.api.nvidia.com/v1` and enables thinking through
//! `extra_body={"chat_template_kwargs": {"thinking": ...}}`.

use serde_json::{json, Map, Value};

use crate::providers::openai::{
    openai_compatible_newtype, AuthStyle, CompatConfig, CompatQuirks, ContextWindowRule,
    OpenAiCompatible, ReasoningModelRule,
};

/// Default base URL for the NVIDIA NIM API.
pub const NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
/// Environment variable holding the NVIDIA NIM API key.
pub const NVIDIA_NIM_API_KEY_ENV: &str = "NVIDIA_NIM_API_KEY";
/// Alternative environment variable for the NVIDIA API key.
pub const NVIDIA_API_KEY_ENV: &str = "NVIDIA_API_KEY";
/// Environment variable overriding the NVIDIA base URL.
pub const NVIDIA_NIM_API_BASE_ENV: &str = "NVIDIA_NIM_API_BASE";
/// Upstream default model.
pub const NVIDIA_DEFAULT_MODEL: &str = "qwen/qwen3.5-122b-a10b";

/// The configuration of the NVIDIA provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "nvidia",
        completions_path: "/chat/completions",
        default_base_url: NVIDIA_NIM_BASE_URL,
        base_url_env: &[NVIDIA_NIM_API_BASE_ENV],
        api_key_env: &[NVIDIA_NIM_API_KEY_ENV, NVIDIA_API_KEY_ENV],
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
    /// NVIDIA NIM chat model, mirroring `ChatNvidia`.
    ChatNvidia
);

impl ChatNvidia {
    /// Mirrors `ChatNvidia.__init__`.
    ///
    /// `thinking` travels as `chat_template_kwargs={"thinking": ...}`.
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
        let extra_body = json!({"chat_template_kwargs": {"thinking": thinking}});

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

    /// The configured `thinking` flag.
    pub fn thinking_flag(&self) -> bool {
        self.inner
            .extra_body
            .as_ref()
            .and_then(|body| body.get("chat_template_kwargs"))
            .and_then(|kwargs| kwargs.get("thinking"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}
