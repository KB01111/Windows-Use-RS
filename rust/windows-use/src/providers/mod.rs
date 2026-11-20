//! LLM provider layer, mirroring `windows_use/providers/`.
//!
//! Every provider talks to its HTTP API directly through `reqwest`; there are no
//! Python SDK bindings in this port. Chat-completions style providers share one
//! generic engine ([`openai::OpenAiCompatible`]); the specialised protocols live
//! in [`anthropic`], [`google`], [`ollama`] and [`perplexity`].
//!
//! ```no_run
//! use windows_use::messages::BaseMessage;
//! use windows_use::providers::{ChatLlm, ChatOpenAI};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let llm = ChatOpenAI::with_model("gpt-4o");
//! let messages = [BaseMessage::human("hello")];
//! let event = llm.invoke(&messages, &[]).await?;
//! println!("{:?}", event.content);
//! # Ok(())
//! # }
//! ```

pub mod anthropic;
pub mod atlas_cloud;
pub mod azure_openai;
pub mod base;
pub mod cerebras;
pub mod cheaper_inference;
pub mod deepseek;
pub mod events;
pub mod google;
pub mod groq;
pub mod litellm;
pub mod mistral;
pub mod nvidia;
pub mod ollama;
pub mod open_router;
pub mod openai;
pub mod perplexity;
pub mod speech;
pub mod tools;
pub mod views;
pub mod vllm;

pub use anthropic::ChatAnthropic;
pub use atlas_cloud::ChatAtlasCloud;
pub use azure_openai::ChatAzureOpenAI;
pub use base::{BaseChatLlm, BoxFuture, ChatLlm, EventStream, SttProvider, TtsProvider};
pub use cerebras::ChatCerebras;
pub use cheaper_inference::ChatCheaperInference;
pub use deepseek::ChatDeepSeek;
pub use events::{LLMEvent, LLMEventType, LLMStreamEvent, LLMStreamEventType, Thinking, ToolCall};
pub use google::ChatGoogle;
pub use groq::ChatGroq;
pub use litellm::ChatLiteLLM;
pub use mistral::ChatMistral;
pub use nvidia::ChatNvidia;
pub use ollama::ChatOllama;
pub use open_router::ChatOpenRouter;
pub use openai::{ChatOpenAI, OpenAiCompatible};
pub use perplexity::ChatPerplexity;
pub use tools::{parse_tool_arguments, sanitize_schema, sanitize_tool_schema, ToolSchema};
pub use views::{Metadata, TokenUsage};
pub use vllm::ChatVLLM;

/// Names of every chat provider this crate implements, matching the Python
/// `providers/__init__.py` export list.
pub const PROVIDER_NAMES: &[&str] = &[
    "anthropic",
    "atlas_cloud",
    "azure_openai",
    "cerebras",
    "cheaper_inference",
    "deepseek",
    "google",
    "groq",
    "litellm",
    "mistral",
    "nvidia",
    "ollama",
    "open_router",
    "openai",
    "perplexity",
    "vllm",
];

/// Which provider an agent should talk to.
///
/// Mirrors the provider switch in the Python configuration layer: the provider
/// name plus the model and optional endpoint/credential overrides.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// Provider id, one of [`PROVIDER_NAMES`].
    pub provider: String,
    /// Model identifier (or Azure deployment name).
    pub model: Option<String>,
    /// API key; falls back to the provider's environment variable.
    pub api_key: Option<String>,
    /// Base URL; falls back to the provider's environment variable or default.
    pub base_url: Option<String>,
    /// Request timeout in seconds.
    pub timeout: Option<f64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Azure API version.
    pub api_version: Option<String>,
}

impl ProviderConfig {
    /// A config with just a provider and model.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: Some(model.into()),
            ..Self::default()
        }
    }
}

/// Builds a boxed provider from a [`ProviderConfig`].
///
/// Construction never performs network access and never fails for a missing API
/// key — the key check happens when `invoke` runs, so a misconfigured provider
/// surfaces as an `Err` from the call rather than a panic at startup.
pub fn create_provider(config: &ProviderConfig) -> anyhow::Result<Box<dyn ChatLlm>> {
    let model = config
        .model
        .clone()
        .unwrap_or_else(|| "gpt-4o".to_string());
    let timeout = config.timeout.unwrap_or(600.0);
    let api_key = config.api_key.clone();
    let base_url = config.base_url.clone();
    let temperature = config.temperature;

    let provider: Box<dyn ChatLlm> = match config.provider.as_str() {
        "openai" => Box::new(ChatOpenAI::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            Some("high".to_string()),
            temperature,
            None,
            serde_json::Map::new(),
        )),
        "anthropic" => Box::new(ChatAnthropic::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            None,
            4096,
            true,
            true,
            true,
            2,
            serde_json::Map::new(),
        )),
        "google" => Box::new(ChatGoogle::new(
            model,
            api_key,
            base_url,
            timeout,
            temperature,
            None,
            None,
            serde_json::Map::new(),
        )),
        "groq" => Box::new(ChatGroq::new(model, api_key, base_url, timeout, 2, temperature, serde_json::Map::new())),
        "mistral" => Box::new(ChatMistral::new(
            model,
            api_key,
            base_url,
            Some(timeout),
            temperature,
            serde_json::Map::new(),
        )),
        "cerebras" => Box::new(ChatCerebras::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            serde_json::Map::new(),
        )),
        "open_router" => Box::new(ChatOpenRouter::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            None,
            serde_json::Map::new(),
        )),
        "azure_openai" => Box::new(ChatAzureOpenAI::new(
            model,
            api_key,
            base_url,
            config.api_version.clone(),
            timeout,
            2,
            temperature,
            serde_json::Map::new(),
        )),
        "litellm" => Box::new(ChatLiteLLM::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            true,
            serde_json::Map::new(),
        )),
        "deepseek" => Box::new(ChatDeepSeek::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            None,
            false,
            serde_json::Map::new(),
        )),
        "nvidia" => Box::new(ChatNvidia::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            false,
            serde_json::Map::new(),
        )),
        "perplexity" => Box::new(ChatPerplexity::new(
            model,
            api_key,
            base_url,
            timeout,
            temperature,
            None,
            serde_json::Map::new(),
        )),
        "vllm" => Box::new(ChatVLLM::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            false,
            serde_json::Map::new(),
        )),
        "atlas_cloud" => Box::new(ChatAtlasCloud::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            serde_json::Map::new(),
        )),
        "cheaper_inference" => Box::new(ChatCheaperInference::new(
            model,
            api_key,
            base_url,
            timeout,
            2,
            temperature,
            serde_json::Map::new(),
        )),
        "ollama" => Box::new(ChatOllama::new(
            model,
            base_url,
            timeout,
            temperature,
            None,
            serde_json::Map::new(),
        )),
        other => anyhow::bail!(
            "unknown provider `{other}`; expected one of {}",
            PROVIDER_NAMES.join(", ")
        ),
    };

    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::BaseMessage;

    #[test]
    fn every_provider_name_builds_without_network_access() {
        for provider in PROVIDER_NAMES {
            let config = ProviderConfig {
                provider: (*provider).to_string(),
                model: Some("test-model".to_string()),
                api_key: Some("test-key".to_string()),
                base_url: Some("https://example.invalid/v1".to_string()),
                timeout: Some(30.0),
                ..ProviderConfig::default()
            };
            let built = create_provider(&config)
                .unwrap_or_else(|error| panic!("provider {provider} failed to build: {error}"));
            // The id round-trips, matching the Python `provider` property.
            assert_eq!(built.provider(), *provider);
            assert_eq!(built.model_name(), "test-model");
            assert!(built.get_metadata().context_window > 0);
        }
    }

    #[test]
    fn unknown_provider_is_an_error_not_a_panic() {
        let config = ProviderConfig::new("not_a_provider", "m");
        assert!(create_provider(&config).is_err());
    }

    #[test]
    fn providers_build_even_without_a_key() {
        // A missing key must not prevent construction; only `invoke` reports it.
        for provider in PROVIDER_NAMES {
            let config = ProviderConfig {
                provider: (*provider).to_string(),
                api_key: Some(String::new()),
                ..ProviderConfig::default()
            };
            let built = create_provider(&config);
            assert!(built.is_ok(), "provider {provider} should construct");
        }
    }

    #[test]
    fn chat_llm_is_object_safe() {
        let provider: Box<dyn ChatLlm> = Box::new(ChatOpenAI::with_model("gpt-4o"));
        let boxed: &dyn ChatLlm = provider.as_ref();
        assert_eq!(boxed.model_name(), "gpt-4o");
        // The default stream adaptor exists for trait objects too.
        let _messages = [BaseMessage::human("hi")];
        let _future = boxed.invoke(&_messages, &[]);
    }
}
