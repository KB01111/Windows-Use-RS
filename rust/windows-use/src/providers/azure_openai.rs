//! Azure OpenAI provider, mirroring `windows_use/providers/azure_openai/llm.py`.
//!
//! Azure differs from plain OpenAI in the URL shape
//! (`{endpoint}/openai/deployments/{deployment}/chat/completions?api-version=...`)
//! and in the `api-key` header instead of a bearer token.

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::messages::BaseMessage;
use crate::providers::base::{
    json_event_stream, require_api_key, resolve_setting, BoxFuture, ChatLlm, EventStream,
    HttpTransport, StreamFraming,
};
use crate::providers::events::LLMEvent;
use crate::providers::openai::{
    accumulate_chunk, finish_stream, parse_usage, AuthStyle, CompatConfig, CompatQuirks,
    ContextWindowRule, OpenAiCompatible, ReasoningModelRule, StreamState,
};
use crate::providers::tools::ToolSchema;
use crate::providers::views::Metadata;

/// Environment variable holding the Azure API key.
pub const AZURE_OPENAI_API_KEY_ENV: &str = "AZURE_OPENAI_API_KEY";
/// Environment variable holding the Azure endpoint.
pub const AZURE_OPENAI_ENDPOINT_ENV: &str = "AZURE_OPENAI_ENDPOINT";
/// Default Azure API version.
pub const AZURE_API_VERSION: &str = "2024-02-01";

/// Builds the Azure-specific configuration.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "azure_openai",
        completions_path: "/chat/completions",
        default_base_url: "",
        base_url_env: &[AZURE_OPENAI_ENDPOINT_ENV],
        api_key_env: &[AZURE_OPENAI_API_KEY_ENV],
        auth: AuthStyle::ApiKeyHeader,
        quirks: CompatQuirks {
            reasoning_as_reasoning_content: true,
            reasoning_model_rule: ReasoningModelRule::OSeriesPrefix,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::Fixed,
        fixed_context_window: 128_000,
        sends_reasoning_effort: false,
    }
}

/// Azure OpenAI chat model, mirroring `ChatAzureOpenAI`.
///
/// `model` plays the role of the Python `deployment_name`, and the deployment
/// segment is what goes on the wire.
#[derive(Debug, Clone)]
pub struct ChatAzureOpenAI {
    /// Deployment name (`deployment_name` in Python).
    pub deployment: String,
    /// Resolved API key.
    pub api_key: Option<String>,
    /// Azure endpoint, e.g. `https://my-resource.openai.azure.com`.
    pub azure_endpoint: Option<String>,
    /// API version query parameter.
    pub api_version: String,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Extra body keys.
    pub extra_body: Option<Value>,
    /// Extra request-body keys (Python `**kwargs`).
    pub kwargs: Map<String, Value>,
    /// Shared engine reused for conversion/parsing.
    engine: OpenAiCompatible,
    transport: HttpTransport,
}

impl ChatAzureOpenAI {
    /// Mirrors `ChatAzureOpenAI.__init__`.
    ///
    /// `api_key`/`azure_endpoint` fall back to `AZURE_OPENAI_API_KEY` and
    /// `AZURE_OPENAI_ENDPOINT`. Construction never fails; a missing key surfaces
    /// as an error from `invoke`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deployment_name: impl Into<String>,
        api_key: Option<String>,
        azure_endpoint: Option<String>,
        api_version: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let deployment = deployment_name.into();
        let api_key = resolve_setting(api_key, &[AZURE_OPENAI_API_KEY_ENV]);
        let azure_endpoint = resolve_setting(azure_endpoint, &[AZURE_OPENAI_ENDPOINT_ENV]);
        let api_version = api_version.unwrap_or_else(|| AZURE_API_VERSION.to_string());

        let engine = OpenAiCompatible::new(
            config(),
            deployment.clone(),
            api_key.clone(),
            azure_endpoint.clone(),
            timeout,
            max_retries,
            None,
            temperature,
            None,
            Map::new(),
        );

        Self {
            deployment,
            api_key,
            azure_endpoint,
            api_version,
            temperature,
            extra_body: None,
            kwargs,
            engine,
            transport: HttpTransport::new(timeout, max_retries),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_deployment(deployment_name: impl Into<String>) -> Self {
        Self::new(deployment_name, None, None, None, 600.0, 2, None, Map::new())
    }

    /// Mirrors `_is_reasoning_model`.
    pub fn is_reasoning_model(&self) -> bool {
        self.deployment.starts_with("o1")
            || self.deployment.starts_with("o3")
            || self.deployment.starts_with("o4")
    }

    /// The chat-completions URL for this deployment.
    pub fn completions_url(&self) -> String {
        let endpoint = self
            .azure_endpoint
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        format!(
            "{endpoint}/openai/deployments/{}/chat/completions?api-version={}",
            self.deployment, self.api_version
        )
    }

    /// Converts messages using the shared OpenAI conversion.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> Vec<Value> {
        self.engine.convert_messages(messages)
    }

    /// Converts tools using the shared OpenAI conversion.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        self.engine.convert_tools(tools)
    }

    /// Assembles the request body.
    pub fn build_params(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
        stream: bool,
    ) -> Map<String, Value> {
        let mut params = Map::new();
        for (key, value) in &self.kwargs {
            params.insert(key.clone(), value.clone());
        }
        params.insert("messages".to_string(), Value::Array(self.convert_messages(messages)));
        if !tools.is_empty() {
            params.insert("tools".to_string(), Value::Array(self.convert_tools(tools)));
        }
        if stream {
            params.insert("stream".to_string(), Value::Bool(true));
            params.insert(
                "stream_options".to_string(),
                serde_json::json!({"include_usage": true}),
            );
        }
        // Reasoning deployments reject temperature.
        if let Some(temperature) = self.temperature {
            if !self.is_reasoning_model() {
                params.insert("temperature".to_string(), serde_json::json!(temperature));
            }
        }
        if json_mode {
            params.insert(
                "response_format".to_string(),
                serde_json::json!({"type": "json_object"}),
            );
        }
        if let Some(Value::Object(extra)) = &self.extra_body {
            for (key, value) in extra {
                params.insert(key.clone(), value.clone());
            }
        }
        params
    }

    /// Parses an Azure response with the shared OpenAI parser.
    pub fn parse_response(&self, response: &Value) -> LLMEvent {
        let mut event = self.engine.parse_response(response);
        // `_process_response` reports usage even for tool calls.
        if event.usage == crate::providers::views::TokenUsage::default() {
            event.usage = parse_usage(response.get("usage"));
        }
        event
    }

    fn ensure_credentials(&self) -> Result<()> {
        require_api_key(&self.api_key, "azure_openai", &[AZURE_OPENAI_API_KEY_ENV])?;
        if self.azure_endpoint.is_none() {
            anyhow::bail!(
                "azure_openai endpoint is not set; pass it to the constructor or set {AZURE_OPENAI_ENDPOINT_ENV}"
            );
        }
        Ok(())
    }
}

impl ChatLlm for ChatAzureOpenAI {
    fn model_name(&self) -> &str {
        &self.deployment
    }

    fn provider(&self) -> &str {
        "azure_openai"
    }

    fn get_metadata(&self) -> Metadata {
        Metadata {
            name: self.deployment.clone(),
            context_window: 128_000,
            owned_by: "azure_openai".to_string(),
        }
    }

    fn invoke<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<LLMEvent>> {
        Box::pin(async move {
            self.ensure_credentials()?;
            let params = self.build_params(messages, tools, false, false);
            let url = self.completions_url();
            let headers = config().auth.headers(self.api_key.as_deref());
            let response = self
                .transport
                .post_json(&url, &headers, &Value::Object(params))
                .await
                .with_context(|| format!("azure_openai request to {url}"))?;
            Ok(self.parse_response(&response))
        })
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<EventStream<'a>>> {
        Box::pin(async move {
            self.ensure_credentials()?;
            let params = self.build_params(messages, tools, false, true);
            let url = self.completions_url();
            let headers = config().auth.headers(self.api_key.as_deref());
            let response = self
                .transport
                .post_stream(&url, &headers, &Value::Object(params))
                .await
                .with_context(|| format!("azure_openai stream from {url}"))?;
            Ok(json_event_stream(
                response,
                StreamFraming::Sse,
                StreamState::default(),
                |state, chunk| accumulate_chunk(state, &chunk, false),
                finish_stream,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_includes_deployment_and_api_version() {
        let provider = ChatAzureOpenAI::new(
            "my-deployment",
            Some("k".to_string()),
            Some("https://example.openai.azure.com/".to_string()),
            None,
            600.0,
            2,
            None,
            Map::new(),
        );
        assert_eq!(
            provider.completions_url(),
            "https://example.openai.azure.com/openai/deployments/my-deployment/chat/completions?api-version=2024-02-01"
        );
        assert_eq!(provider.get_metadata().context_window, 128_000);
        assert_eq!(provider.provider(), "azure_openai");
        assert_eq!(provider.model_name(), "my-deployment");
    }

    #[test]
    fn reasoning_deployments_skip_temperature() {
        let provider = ChatAzureOpenAI::new(
            "o1-preview",
            Some("k".to_string()),
            Some("https://example.openai.azure.com".to_string()),
            None,
            600.0,
            2,
            Some(0.7),
            Map::new(),
        );
        let params = provider.build_params(&[BaseMessage::human("hi")], &[], false, false);
        assert!(!params.contains_key("temperature"));
        assert!(!params.contains_key("model"));
        assert_eq!(params["messages"][0]["content"], "hi");
    }

    #[test]
    fn missing_endpoint_errors_on_use() {
        let provider = ChatAzureOpenAI::new(
            "d",
            Some("k".to_string()),
            Some(String::new()),
            None,
            600.0,
            2,
            None,
            Map::new(),
        );
        assert!(provider.ensure_credentials().is_err());
    }

    #[test]
    fn tool_calls_parse_through_the_shared_engine() {
        let provider = ChatAzureOpenAI::with_deployment("gpt-4o");
        let event = provider.parse_response(&json!({
            "choices": [{"message": {"tool_calls": [
                {"id": "c1", "function": {"name": "click", "arguments": "{\"x\":2}"}}
            ]}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
        }));
        assert!(event.is_tool_call());
        assert_eq!(event.tool_call.unwrap().params.get("x"), Some(&json!(2)));
        assert_eq!(event.usage.total_tokens, 3);
    }
}
