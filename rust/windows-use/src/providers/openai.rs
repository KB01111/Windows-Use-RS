//! OpenAI Chat Completions provider plus the shared engine used by every
//! OpenAI-compatible provider.
//!
//! Mirrors `windows_use/providers/openai/llm.py`. The same conversion and parsing
//! code serves `groq`, `mistral`, `cerebras`, `open_router`, `azure_openai`,
//! `litellm`, `deepseek`, `nvidia`, `vllm`, `atlas_cloud` and
//! `cheaper_inference`: they differ only in endpoint, auth header style, env var
//! names and a few model-specific switches, all captured by [`CompatConfig`] and
//! [`CompatQuirks`]. Each of those providers owns a thin `ChatXxx` newtype that
//! delegates to [`OpenAiCompatible`] through [`delegate_chat_llm`].

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::messages::BaseMessage;
use crate::providers::base::{
    first_string_field, json_i64, json_path_i64, require_api_key, resolve_setting,
    ChatLlm, BoxFuture, EventStream, HttpTransport, StreamFraming,
};
use crate::providers::events::{LLMEvent, LLMEventType, LLMStreamEvent, LLMStreamEventType, Thinking, ToolCall};
use crate::providers::tools::{parse_tool_arguments, parse_tool_arguments_str, serialize_tool_arguments, ToolSchema};
use crate::providers::views::{Metadata, TokenUsage};
use crate::providers::base::json_event_stream;

/// Base URL used when the caller and the environment provide none.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Environment variable holding the OpenAI API key.
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
/// Environment variable holding an OpenAI-compatible base URL.
pub const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
/// Upstream default model for [`ChatOpenAI`].
pub const OPENAI_DEFAULT_MODEL: &str = "gpt-4o";

/// How a provider authenticates its requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>` — OpenAI and most compatible gateways.
    Bearer,
    /// `api-key: <key>` — Azure OpenAI.
    ApiKeyHeader,
    /// Bearer auth plus extra headers, e.g. OpenRouter's `HTTP-Referer`.
    BearerWithHeaders(Vec<(String, String)>),
}

impl AuthStyle {
    /// Renders the header list for a request.
    ///
    /// A missing key yields no auth header so the request still goes out and the
    /// server reports the failure — matching a Python client built with
    /// `api_key=None`.
    pub fn headers(&self, api_key: Option<&str>) -> Vec<(String, String)> {
        let mut headers = vec![(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )];
        let key = api_key.filter(|key| !key.is_empty());
        match self {
            AuthStyle::Bearer => {
                if let Some(key) = key {
                    headers.push(("Authorization".to_string(), format!("Bearer {key}")));
                }
            }
            AuthStyle::ApiKeyHeader => {
                if let Some(key) = key {
                    headers.push(("api-key".to_string(), key.to_string()));
                }
            }
            AuthStyle::BearerWithHeaders(extra) => {
                if let Some(key) = key {
                    headers.push(("Authorization".to_string(), format!("Bearer {key}")));
                }
                headers.extend(extra.iter().cloned());
            }
        }
        headers
    }
}

/// Which models a provider treats as reasoning models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReasoningModelRule {
    /// Never a reasoning model (Groq, Mistral, ...).
    #[default]
    Never,
    /// `o1`/`o3`/`o4` prefixes, like `ChatOpenAI._is_reasoning_model`.
    OSeriesPrefix,
    /// Substring match on a provider-supplied needle list.
    Contains,
    /// Contains `"reasoner"` (DeepSeek).
    ReasonerSubstring,
}

impl ReasoningModelRule {
    /// Whether `model` counts as a reasoning model under this rule.
    pub fn matches(&self, model: &str, needles: &[&str]) -> bool {
        match self {
            ReasoningModelRule::Never => false,
            ReasoningModelRule::OSeriesPrefix => {
                model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4")
            }
            ReasoningModelRule::Contains => {
                let lowered = model.to_lowercase();
                needles.iter().any(|needle| lowered.contains(needle))
            }
            ReasoningModelRule::ReasonerSubstring => model.to_lowercase().contains("reasoner"),
        }
    }
}

/// How a provider derives its context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextWindowRule {
    /// The `gpt-4`/`gpt-4-turbo`/`gpt-3.5`/`o1` table, default 128000.
    OpenAiFamily,
    /// Always [`CompatConfig::fixed_context_window`].
    Fixed,
    /// Mistral's large/medium/small/codestral table.
    MistralFamily,
    /// Groq's llama/mixtral/gemma table.
    GroqFamily,
}

/// Provider-specific switches layered on the shared OpenAI conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompatQuirks {
    /// Serialize reasoning as `reasoning_content` (OpenAI o-series, Azure, vLLM,
    /// OpenRouter) rather than `reasoning` (Groq).
    pub reasoning_as_reasoning_content: bool,
    /// Embed thinking inline in the assistant text (Cerebras): GPT-OSS prefixes
    /// it, everything else wraps it in `<think>` tags.
    pub inline_thinking: bool,
    /// Mistral: rewrite tool-call ids to 9 alphanumerics and pass thinking as
    /// `{"type": "thinking"}` content chunks.
    pub mistral_content_chunks: bool,
    /// Groq: ask for parsed reasoning on Qwen, `include_reasoning` otherwise.
    pub groq_reasoning_params: bool,
    /// Also read reasoning from `message.reasoning`.
    pub accept_reasoning_field: bool,
    /// Which models the provider treats as reasoning models.
    pub reasoning_model_rule: ReasoningModelRule,
}

/// Everything that differs between OpenAI-compatible providers.
#[derive(Debug, Clone)]
pub struct CompatConfig {
    /// Provider id returned by [`ChatLlm::provider`].
    pub provider: &'static str,
    /// Chat-completions path appended to the base URL.
    pub completions_path: &'static str,
    /// Default base URL when neither the constructor nor the environment sets one.
    pub default_base_url: &'static str,
    /// Environment variables consulted for the base URL.
    pub base_url_env: &'static [&'static str],
    /// Environment variables consulted for the API key.
    pub api_key_env: &'static [&'static str],
    /// How the key is attached to requests.
    pub auth: AuthStyle,
    /// Provider-specific conversion switches.
    pub quirks: CompatQuirks,
    /// Model-name needles for [`ReasoningModelRule::Contains`].
    pub reasoning_needles: &'static [&'static str],
    /// Selection rule for the context window.
    pub context_window: ContextWindowRule,
    /// Static context window when the rule is [`ContextWindowRule::Fixed`].
    pub fixed_context_window: i64,
    /// Always send `reasoning_effort` when the model is a reasoning model.
    pub sends_reasoning_effort: bool,
}

/// The shared OpenAI-compatible chat engine.
///
/// Constructed by the per-provider `ChatXxx::new` helpers; not usually used
/// directly, though it is public so callers can build custom gateways.
#[derive(Debug, Clone)]
pub struct OpenAiCompatible {
    /// Configured model id.
    pub model: String,
    /// Resolved API key (`None` when the environment does not provide one).
    pub api_key: Option<String>,
    /// Base URL without a trailing slash.
    pub base_url: String,
    /// Sampling temperature, when the caller asked for one.
    pub temperature: Option<f64>,
    /// Reasoning effort for o-series models.
    pub reasoning_effort: Option<String>,
    /// Provider-specific body keys (`extra_body` in Python).
    pub extra_body: Option<Value>,
    /// Extra request-body keys (Python's `**kwargs`).
    pub kwargs: Map<String, Value>,
    /// Provider configuration.
    pub config: CompatConfig,
    transport: HttpTransport,
}

impl OpenAiCompatible {
    /// Builds the engine from the same arguments the Python classes accept.
    ///
    /// Unset `api_key`/`base_url` fall back to the provider's environment
    /// variables. `kwargs` are merged into the request body and may carry a
    /// `thinking` flag for template-driven providers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CompatConfig,
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        reasoning_effort: Option<String>,
        temperature: Option<f64>,
        extra_body: Option<Value>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let api_key = resolve_setting(api_key, config.api_key_env);
        let base_url = resolve_setting(base_url, config.base_url_env)
            .unwrap_or_else(|| config.default_base_url.to_string());

        Self {
            model: model.into(),
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            temperature,
            reasoning_effort,
            extra_body,
            kwargs,
            config,
            transport: HttpTransport::new(timeout, max_retries),
        }
    }

    /// The configured base URL (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The resolved API key, if any.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// The model id.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// The provider id.
    pub fn provider(&self) -> &str {
        self.config.provider
    }

    /// The transport, exposed for providers needing a bespoke call.
    pub fn transport(&self) -> &HttpTransport {
        &self.transport
    }

    /// Whether the provider was constructed with a thinking flag.
    pub fn thinking_enabled(&self) -> bool {
        self.kwargs
            .get("thinking")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Mirrors `ChatOpenAI._is_reasoning_model`.
    pub fn is_reasoning_model(&self) -> bool {
        self.config
            .quirks
            .reasoning_model_rule
            .matches(&self.model, self.config.reasoning_needles)
    }

    /// The full chat-completions URL.
    pub fn completions_url(&self) -> String {
        format!("{}{}", self.base_url, self.config.completions_path)
    }

    /// Validates that a request can be attempted.
    pub fn ensure_credentials(&self) -> Result<()> {
        require_api_key(&self.api_key, self.config.provider, self.config.api_key_env)
    }

    fn accepts_temperature(&self) -> bool {
        self.temperature.is_some() && !self.is_reasoning_model()
    }

    /// Merges the `extra_body` keys inline; the Python SDK's `extra_body`
    /// envelope has no HTTP analogue, so the keys become part of the body.
    fn inline_extra_body(&self, params: &mut Map<String, Value>) {
        if let Some(Value::Object(extra)) = &self.extra_body {
            for (key, value) in extra {
                params.insert(key.clone(), value.clone());
            }
        }
    }

    /// Adds the provider-specific request keys.
    fn apply_provider_params(&self, params: &mut Map<String, Value>) {
        let quirks = &self.config.quirks;
        let reasoning = self.is_reasoning_model();

        if quirks.groq_reasoning_params && reasoning {
            if self.model.to_lowercase().contains("qwen") {
                params.insert("reasoning_format".to_string(), json!("parsed"));
            } else {
                params.insert("include_reasoning".to_string(), json!(true));
            }
        }

        if self.config.sends_reasoning_effort && reasoning {
            if let Some(effort) = self.reasoning_effort.as_deref() {
                params
                    .entry("reasoning_effort".to_string())
                    .or_insert_with(|| json!(effort));
            }
        }

        // vLLM and NIM read the flag from the chat template kwargs.
        if self.thinking_enabled() {
            let template = params
                .get("chat_template_kwargs")
                .cloned()
                .unwrap_or_else(|| json!({"thinking": true}));
            params.insert("chat_template_kwargs".to_string(), template);
        }
    }

    /// Converts messages to OpenAI wire format.
    ///
    /// Mirrors `ChatOpenAI._convert_messages`, including `reasoning_content`
    /// replay for reasoning models and tool-call/tool-result reconstruction.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> Vec<Value> {
        let quirks = &self.config.quirks;
        let mut converted: Vec<Value> = Vec::with_capacity(messages.len());

        for message in messages {
            match message {
                BaseMessage::SystemMessage { content } => {
                    converted.push(json!({"role": "system", "content": content}));
                }
                BaseMessage::HumanMessage { content } => {
                    converted.push(json!({"role": "user", "content": content}));
                }
                BaseMessage::ImageMessage { .. } => {
                    let content = message.content().unwrap_or_default();
                    let mut content_list: Vec<Value> = Vec::new();
                    if !content.is_empty() {
                        content_list.push(json!({"type": "text", "text": content}));
                    }
                    let (encoded, mime_type) = message.convert_images_base64_with_mime();
                    for payload in encoded {
                        content_list.push(json!({
                            "type": "image_url",
                            "image_url": {"url": format!("data:{mime_type};base64,{payload}")},
                        }));
                    }
                    converted.push(json!({"role": "user", "content": content_list}));
                }
                BaseMessage::AIMessage { content, thinking, .. } => {
                    converted.push(self.assistant_turn(content.as_deref(), thinking.as_deref(), None));
                }
                BaseMessage::ToolMessage {
                    id,
                    name,
                    params,
                    content,
                    thinking,
                    ..
                } => {
                    let tool_id = if quirks.mistral_content_chunks {
                        mistral_tool_call_id(id)
                    } else {
                        id.clone()
                    };
                    let tool_call = json!({
                        "id": tool_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serialize_tool_arguments(params),
                        },
                    });
                    converted.push(self.assistant_turn(
                        None,
                        thinking.as_deref(),
                        Some(tool_call),
                    ));

                    let mut result = Map::new();
                    result.insert("role".to_string(), json!("tool"));
                    result.insert("tool_call_id".to_string(), json!(tool_id));
                    if quirks.mistral_content_chunks {
                        result.insert("name".to_string(), json!(name));
                    }
                    result.insert(
                        "content".to_string(),
                        json!(content.clone().unwrap_or_default()),
                    );
                    converted.push(Value::Object(result));
                }
            }
        }

        converted
    }

    /// Builds one assistant turn, honouring the provider's thinking convention.
    fn assistant_turn(
        &self,
        content: Option<&str>,
        thinking: Option<&str>,
        tool_call: Option<Value>,
    ) -> Value {
        let quirks = &self.config.quirks;
        let mut entry = Map::new();
        entry.insert("role".to_string(), json!("assistant"));

        if quirks.mistral_content_chunks && thinking.is_some() {
            // Magistral models: thinking travels as content chunks.
            let mut chunks: Vec<Value> = vec![json!({
                "type": "thinking",
                "thinking": [{"type": "text", "text": thinking.unwrap_or_default()}],
            })];
            if let Some(text) = content.filter(|text| !text.is_empty()) {
                chunks.push(json!({"type": "text", "text": text}));
            }
            entry.insert("content".to_string(), Value::Array(chunks));
        } else if quirks.inline_thinking {
            entry.insert(
                "content".to_string(),
                json!(format_assistant_content(&self.model, content, thinking)),
            );
        } else {
            entry.insert(
                "content".to_string(),
                match content {
                    Some(text) => json!(text),
                    None => Value::Null,
                },
            );
            if let Some(thinking) = thinking {
                let key = if quirks.reasoning_as_reasoning_content {
                    "reasoning_content"
                } else {
                    "reasoning"
                };
                entry.insert(key.to_string(), json!(thinking));
            }
        }

        if let Some(tool_call) = tool_call {
            entry.insert("tool_calls".to_string(), json!([tool_call]));
        }

        Value::Object(entry)
    }

    /// Converts tools to the OpenAI `{"type": "function", "function": ...}` shape.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        tools.iter().map(ToolSchema::to_openai_tool).collect()
    }

    /// Assembles the request body. `stream` adds the streaming keys, exactly like
    /// the Python `invoke`/`stream` variants.
    pub fn build_params(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
        stream: bool,
    ) -> Map<String, Value> {
        let mut params: Map<String, Value> = Map::new();

        // Python's `**self.kwargs` is spread first so explicit keys win.
        for (key, value) in &self.kwargs {
            if key == "thinking" {
                continue;
            }
            params.insert(key.clone(), value.clone());
        }

        params.insert("model".to_string(), json!(self.model));
        params.insert("messages".to_string(), json!(self.convert_messages(messages)));

        if !tools.is_empty() {
            params.insert("tools".to_string(), json!(self.convert_tools(tools)));
        }

        if stream {
            params.insert("stream".to_string(), json!(true));
            params.insert("stream_options".to_string(), json!({"include_usage": true}));
        }

        if self.accepts_temperature() {
            params.insert("temperature".to_string(), json!(self.temperature));
        }

        if json_mode {
            params.insert("response_format".to_string(), json!({"type": "json_object"}));
        }

        self.apply_provider_params(&mut params);
        self.inline_extra_body(&mut params);
        params
    }

    /// Parses a chat-completions response into an [`LLMEvent`].
    ///
    /// Mirrors `ChatOpenAI._process_response`: the first choice wins, reasoning
    /// tokens come from `completion_tokens_details`, and malformed tool arguments
    /// become an empty map rather than an error.
    pub fn parse_response(&self, response: &Value) -> LLMEvent {
        let choice = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .cloned()
            .unwrap_or(Value::Null);
        let message = choice.get("message").cloned().unwrap_or(Value::Null);

        let usage = parse_usage(response.get("usage"));
        let thinking = self.extract_thinking(&message, &choice);

        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            if let Some(tool_call) = tool_calls.first() {
                let function = tool_call.get("function").cloned().unwrap_or(Value::Null);
                let params = parse_tool_arguments(function.get("arguments"));
                return LLMEvent::tool_call(ToolCall {
                    id: tool_call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: function
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    params,
                })
                .with_thinking(thinking)
                .with_usage(usage);
            }
        }

        let content = match message.get("content") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };

        LLMEvent {
            event_type: LLMEventType::Text,
            thinking,
            content: Some(content),
            tool_call: None,
            usage,
        }
    }

    /// Reads reasoning content, honouring the provider's preferred field names.
    fn extract_thinking(&self, message: &Value, choice: &Value) -> Option<Thinking> {
        let mut candidates: Vec<&str> = vec!["reasoning_content"];
        if self.config.quirks.accept_reasoning_field {
            candidates.insert(0, "reasoning");
        }
        first_string_field(message, &candidates)
            .or_else(|| first_string_field(choice, &["reasoning_content", "reasoning"]))
            .map(Thinking::plain)
    }

    /// POSTs once and returns the decoded JSON reply.
    pub async fn post(&self, body: &Map<String, Value>) -> Result<Value> {
        self.ensure_credentials()?;
        let url = self.completions_url();
        let headers = self.config.auth.headers(self.api_key.as_deref());
        self.transport
            .post_json(&url, &headers, &Value::Object(body.clone()))
            .await
            .with_context(|| format!("{} request to {url}", self.config.provider))
    }

    /// Non-streaming call.
    pub async fn invoke_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<LLMEvent> {
        let params = self.build_params(messages, tools, json_mode, false);
        let response = self.post(&params).await?;
        Ok(self.parse_response(&response))
    }

    /// Streaming call, delegating chunk handling to [`accumulate_chunk`].
    pub async fn stream_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<EventStream<'static>> {
        self.ensure_credentials()?;
        let params = self.build_params(messages, tools, json_mode, true);
        let url = self.completions_url();
        let headers = self.config.auth.headers(self.api_key.as_deref());
        let response = self
            .transport
            .post_stream(&url, &headers, &Value::Object(params))
            .await
            .with_context(|| format!("{} stream from {url}", self.config.provider))?;

        let inline_thinking = self.config.quirks.inline_thinking;
        Ok(json_event_stream(
            response,
            StreamFraming::Sse,
            StreamState::default(),
            move |state, chunk| accumulate_chunk(state, &chunk, inline_thinking),
            finish_stream,
        ))
    }

    /// Context-window metadata for the configured model.
    pub fn metadata(&self) -> Metadata {
        let context_window = match self.config.context_window {
            ContextWindowRule::OpenAiFamily => openai_context_window(&self.model),
            ContextWindowRule::Fixed => self.config.fixed_context_window,
            ContextWindowRule::MistralFamily => mistral_context_window(&self.model),
            ContextWindowRule::GroqFamily => groq_context_window(&self.model),
        };
        Metadata {
            name: self.model.clone(),
            context_window,
            owned_by: self.config.provider.to_string(),
        }
    }
}

/// `ChatOpenAI`'s context-window table.
///
/// Default 128000; `gpt-4-turbo*` 128000, `gpt-4*` 8192, `gpt-3.5-turbo*` 16385,
/// `o1*` 200000 — the exact order and prefixes of
/// `ChatOpenAI.get_metadata`.
///
/// Note the prefixes are not mutually exclusive: because `gpt-4` is tested before
/// `o1` and without an end boundary, `gpt-4o` resolves to 8192 in the Python
/// source as well. This port keeps that behaviour so budgets match upstream.
pub fn openai_context_window(model: &str) -> i64 {
    if model.starts_with("gpt-4-turbo") {
        128_000
    } else if model.starts_with("gpt-4") {
        8_192
    } else if model.starts_with("gpt-3.5-turbo") {
        16_385
    } else if model.starts_with("o1") {
        200_000
    } else {
        128_000
    }
}

fn mistral_context_window(model: &str) -> i64 {
    if model.contains("large") {
        128_000
    } else if model.contains("medium") || model.contains("small") || model.contains("codestral") {
        32_000
    } else {
        128_000
    }
}

fn groq_context_window(model: &str) -> i64 {
    if model.contains("llama-3.3") || model.contains("llama-3.1") {
        128_000
    } else if model.contains("mixtral") {
        32_768
    } else if model.contains("gemma-2") {
        8_192
    } else {
        128_000
    }
}

/// Parses a chat-completions `usage` object.
pub fn parse_usage(usage: Option<&Value>) -> TokenUsage {
    let Some(usage) = usage else {
        return TokenUsage::default();
    };
    let thinking_tokens = json_path_i64(usage, "completion_tokens_details.reasoning_tokens")
        .or_else(|| json_path_i64(usage, "completion_tokens_details.thinking_tokens"))
        .or_else(|| json_path_i64(usage, "prompt_tokens_details.reasoning_tokens"));

    TokenUsage {
        prompt_tokens: json_i64(usage.get("prompt_tokens")).unwrap_or(0),
        completion_tokens: json_i64(usage.get("completion_tokens")).unwrap_or(0),
        total_tokens: json_i64(usage.get("total_tokens")).unwrap_or(0),
        image_tokens: 0,
        thinking_tokens: thinking_tokens.unwrap_or(0),
        cache_creation_input_tokens: json_i64(usage.get("cache_creation_input_tokens"))
            .unwrap_or(0),
        cache_read_input_tokens: json_i64(usage.get("cache_read_input_tokens"))
            .or_else(|| json_i64(usage.get("prompt_cache_hit_tokens")))
            .unwrap_or(0),
    }
}

/// `_format_assistant_content` from the Cerebras provider.
pub fn format_assistant_content(
    model: &str,
    content: Option<&str>,
    thinking: Option<&str>,
) -> String {
    let text = content.unwrap_or_default();
    let Some(thinking) = thinking else {
        return text.to_string();
    };
    if model.to_lowercase().contains("gpt-oss") {
        if text.is_empty() {
            thinking.to_string()
        } else {
            format!("{thinking} {text}")
        }
    } else if text.is_empty() {
        format!("<think>{thinking}</think>")
    } else {
        format!("<think>{thinking}</think>{text}")
    }
}

/// `_mistral_tool_call_id`: exactly 9 alphanumeric characters, zero padded.
pub fn mistral_tool_call_id(raw: &str) -> String {
    let alphanumeric: String = raw
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect();
    if alphanumeric.chars().count() >= 9 {
        alphanumeric.chars().take(9).collect()
    } else {
        let padding = 9 - alphanumeric.chars().count();
        format!("{alphanumeric}{}", "0".repeat(padding))
    }
}

/// Accumulator state for streamed chat-completions chunks.
#[derive(Debug, Default)]
pub struct StreamState {
    pub text_started: bool,
    pub think_started: bool,
    pub tool_call_id: Option<String>,
    pub tool_call_name: Option<String>,
    pub tool_call_args: String,
    pub usage: Option<TokenUsage>,
}

/// Folds one streamed chunk into `state`, returning the events it produced.
pub fn accumulate_chunk(
    state: &mut StreamState,
    chunk: &Value,
    inline_thinking: bool,
) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();

    let choices = chunk.get("choices").and_then(Value::as_array);
    let has_choices = choices.map(|choices| !choices.is_empty()).unwrap_or(false);

    if !has_choices {
        // Final usage-only chunk.
        if let Some(usage) = chunk.get("usage") {
            state.usage = Some(parse_usage(Some(usage)));
        }
        return events;
    }

    let first = choices.and_then(|choices| choices.first()).cloned().unwrap_or(Value::Null);
    let delta = first.get("delta").cloned().unwrap_or(Value::Null);

    // Reasoning arrives as `reasoning_content` or `reasoning` depending on provider.
    if let Some(reasoning) = first_string_field(&delta, &["reasoning_content", "reasoning"]) {
        if !state.think_started {
            state.think_started = true;
            events.push(LLMStreamEvent::think_start());
        }
        events.push(LLMStreamEvent::think_delta(reasoning));
    }

    if let Some(content) = delta.get("content").and_then(Value::as_str) {
        if !content.is_empty() {
            if state.think_started {
                state.think_started = false;
                events.push(LLMStreamEvent::think_end());
            }
            if !state.text_started {
                state.text_started = true;
                let _ = inline_thinking;
                events.push(LLMStreamEvent::text_start());
            }
            events.push(LLMStreamEvent::text_delta(content));
        }
    }

    // Tool-call deltas: only the first call, matching the Python accumulator.
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        if let Some(tool_call) = tool_calls.first() {
            if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    state.tool_call_id = Some(id.to_string());
                }
            }
            if let Some(function) = tool_call.get("function") {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    if !name.is_empty() {
                        state.tool_call_name = Some(name.to_string());
                    }
                }
                if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                    state.tool_call_args.push_str(arguments);
                }
            }
        }
    }

    events
}

/// Emits the closing events once the stream is exhausted.
pub fn finish_stream(state: &mut StreamState) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();
    if state.think_started {
        state.think_started = false;
        events.push(LLMStreamEvent::think_end());
    }

    match (state.tool_call_id.clone(), state.tool_call_name.clone()) {
        (Some(id), Some(name)) => {
            let params = parse_tool_arguments_str(&state.tool_call_args);
            events.push(
                LLMStreamEvent::new(LLMStreamEventType::ToolCall)
                    .with_tool_call(ToolCall { id, name, params })
                    .with_usage(state.usage),
            );
        }
        _ => {
            if state.text_started {
                state.text_started = false;
                events.push(LLMStreamEvent::text_end(state.usage));
            }
        }
    }

    events
}

/// The configuration of the plain OpenAI provider.
pub fn openai_config() -> CompatConfig {
    CompatConfig {
        provider: "openai",
        completions_path: "/chat/completions",
        default_base_url: OPENAI_BASE_URL,
        base_url_env: &[OPENAI_BASE_URL_ENV],
        api_key_env: &[OPENAI_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            reasoning_as_reasoning_content: true,
            reasoning_model_rule: ReasoningModelRule::OSeriesPrefix,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::OpenAiFamily,
        fixed_context_window: 128_000,
        sends_reasoning_effort: true,
    }
}

/// Generates an OpenAI-compatible provider newtype plus its [`ChatLlm`] impl.
///
/// The generated type owns a public `inner: OpenAiCompatible` field and gets
/// `config()`, `inner()` and `completions_url()` helpers; each provider module
/// adds its own `new(...)` with the Python constructor arguments.
macro_rules! openai_compatible_newtype {
    (
        $(#[$meta:meta])*
        $struct_name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $struct_name {
            /// The shared OpenAI-compatible engine.
            pub inner: $crate::providers::openai::OpenAiCompatible,
        }

        impl $struct_name {
            /// The provider configuration for this backend.
            pub fn config() -> $crate::providers::openai::CompatConfig {
                config()
            }

            /// The shared engine, for custom flows and tests.
            pub fn inner(&self) -> &$crate::providers::openai::OpenAiCompatible {
                &self.inner
            }

            /// The full chat-completions URL.
            pub fn completions_url(&self) -> String {
                self.inner.completions_url()
            }

            /// Non-streaming call with Python's `json_mode` flag.
            pub async fn invoke_json(
                &self,
                messages: &[$crate::messages::BaseMessage],
                tools: &[$crate::providers::tools::ToolSchema],
                json_mode: bool,
            ) -> anyhow::Result<$crate::providers::events::LLMEvent> {
                self.inner.invoke_messages(messages, tools, json_mode).await
            }

            /// Streaming call with Python's `json_mode` flag.
            pub async fn stream_json(
                &self,
                messages: &[$crate::messages::BaseMessage],
                tools: &[$crate::providers::tools::ToolSchema],
                json_mode: bool,
            ) -> anyhow::Result<$crate::providers::base::EventStream<'static>> {
                self.inner.stream_messages(messages, tools, json_mode).await
            }
        }

        $crate::providers::openai::impl_chat_llm_inner!($struct_name);
    };
}

/// Implements [`ChatLlm`] by delegating to `self.inner`.
macro_rules! impl_chat_llm_inner {
    ($struct_name:ident) => {
        impl $crate::providers::base::ChatLlm for $struct_name {
            fn model_name(&self) -> &str {
                self.inner.model_name()
            }

            fn provider(&self) -> &str {
                self.inner.provider()
            }

            fn get_metadata(&self) -> $crate::providers::views::Metadata {
                self.inner.metadata()
            }

            fn invoke<'a>(
                &'a self,
                messages: &'a [$crate::messages::BaseMessage],
                tools: &'a [$crate::providers::tools::ToolSchema],
            ) -> $crate::providers::base::BoxFuture<
                'a,
                anyhow::Result<$crate::providers::events::LLMEvent>,
            > {
                Box::pin(async move { self.inner.invoke_messages(messages, tools, false).await })
            }

            fn stream<'a>(
                &'a self,
                messages: &'a [$crate::messages::BaseMessage],
                tools: &'a [$crate::providers::tools::ToolSchema],
            ) -> $crate::providers::base::BoxFuture<
                'a,
                anyhow::Result<$crate::providers::base::EventStream<'a>>,
            > {
                Box::pin(async move { self.inner.stream_messages(messages, tools, false).await })
            }
        }
    };
}

pub(crate) use impl_chat_llm_inner;
pub(crate) use openai_compatible_newtype;

/// OpenAI chat model, mirroring `ChatOpenAI`.
#[derive(Debug, Clone)]
pub struct ChatOpenAI {
    /// The shared engine.
    pub inner: OpenAiCompatible,
}

impl ChatOpenAI {
    /// Mirrors `ChatOpenAI.__init__`.
    ///
    /// `timeout` is in seconds (`600.0` in Python), `max_retries` defaults to `2`,
    /// `reasoning_effort` only travels to o-series models, and `extra_body` plus
    /// `kwargs` are merged into the request body.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        reasoning_effort: Option<String>,
        temperature: Option<f64>,
        extra_body: Option<Value>,
        kwargs: Map<String, Value>,
    ) -> Self {
        Self {
            inner: OpenAiCompatible::new(
                openai_config(),
                model,
                api_key,
                base_url,
                timeout,
                max_retries,
                reasoning_effort,
                temperature,
                extra_body,
                kwargs,
            ),
        }
    }

    /// Constructor with the Python defaults (`gpt-4o`, 600s, 2 retries, effort high).
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(
            model,
            None,
            None,
            600.0,
            2,
            Some("high".to_string()),
            None,
            None,
            Map::new(),
        )
    }

    /// The shared engine, for tests and custom flows.
    pub fn inner(&self) -> &OpenAiCompatible {
        &self.inner
    }

    /// The upstream default model.
    pub fn default_model() -> &'static str {
        OPENAI_DEFAULT_MODEL
    }

    /// Non-streaming call with Python's `json_mode` flag.
    pub async fn invoke_json(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<LLMEvent> {
        self.inner.invoke_messages(messages, tools, json_mode).await
    }

    /// Streaming call with Python's `json_mode` flag.
    pub async fn stream_json(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<EventStream<'static>> {
        self.inner.stream_messages(messages, tools, json_mode).await
    }
}

impl_chat_llm_inner!(ChatOpenAI);

/// Convenience: invoke through a `dyn ChatLlm`.
pub fn boxed_invoke<'a>(
    provider: &'a dyn ChatLlm,
    messages: &'a [BaseMessage],
    tools: &'a [ToolSchema],
) -> BoxFuture<'a, Result<LLMEvent>> {
    provider.invoke(messages, tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Usage;

    fn provider() -> ChatOpenAI {
        ChatOpenAI::new(
            "gpt-4o",
            Some("test-key".to_string()),
            Some("https://example.invalid/v1".to_string()),
            600.0,
            2,
            Some("high".to_string()),
            None,
            None,
            Map::new(),
        )
    }

    #[test]
    fn message_conversion_matches_openai_wire_format() {
        let provider = provider();
        let messages = vec![
            BaseMessage::system("be terse"),
            BaseMessage::human("hello"),
            BaseMessage::AIMessage {
                content: Some("hi".to_string()),
                thinking: Some("pondering".to_string()),
                thinking_signature: None,
                usage: Some(Usage::new(1, 2, 3)),
            },
            BaseMessage::ToolMessage {
                id: "call_1".to_string(),
                name: "click".to_string(),
                params: serde_json::json!({"x": 1}).as_object().cloned().unwrap(),
                content: Some("done".to_string()),
                thinking: Some("why".to_string()),
                thinking_signature: None,
                usage: None,
            },
        ];

        let converted = provider.inner().convert_messages(&messages);

        assert_eq!(converted[0], json!({"role": "system", "content": "be terse"}));
        assert_eq!(converted[1], json!({"role": "user", "content": "hello"}));
        assert_eq!(converted[2]["role"], "assistant");
        assert_eq!(converted[2]["content"], "hi");
        assert_eq!(converted[2]["reasoning_content"], "pondering");
        // Tool call reconstruction: an assistant turn followed by the tool result.
        assert_eq!(converted[3]["role"], "assistant");
        assert_eq!(converted[3]["tool_calls"][0]["id"], "call_1");
        assert_eq!(converted[3]["tool_calls"][0]["function"]["name"], "click");
        assert_eq!(converted[3]["tool_calls"][0]["function"]["arguments"], "{\"x\":1}");
        assert_eq!(converted[3]["reasoning_content"], "why");
        assert_eq!(
            converted[4],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "done"})
        );
    }

    #[test]
    fn image_message_converts_to_data_url_parts() {
        use image::ImageEncoder as _;

        let provider = provider();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[10u8, 20, 30], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();

        let message = BaseMessage::ImageMessage {
            content: "look".to_string(),
            images: vec![png],
            mime_type: "image/png".to_string(),
        };
        let converted = provider.inner().convert_messages(&[message]);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"][0], json!({"type": "text", "text": "look"}));
        let url = converted[0]["content"][1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn reasoning_models_skip_temperature_and_request_effort() {
        let reasoning = ChatOpenAI::new(
            "o1-mini",
            Some("k".to_string()),
            None,
            600.0,
            2,
            Some("high".to_string()),
            Some(0.5),
            None,
            Map::new(),
        );
        let params = reasoning
            .inner()
            .build_params(&[BaseMessage::human("hi")], &[], false, false);
        assert!(!params.contains_key("temperature"));
        assert_eq!(params["reasoning_effort"], "high");

        let plain = ChatOpenAI::new(
            "gpt-4o",
            Some("k".to_string()),
            None,
            600.0,
            2,
            Some("high".to_string()),
            Some(0.5),
            None,
            Map::new(),
        );
        let params = plain
            .inner()
            .build_params(&[BaseMessage::human("hi")], &[], false, false);
        assert_eq!(params["temperature"], 0.5);
    }

    #[test]
    fn response_parsing_extracts_text_and_usage() {
        let provider = provider();
        let response = json!({
            "choices": [{"message": {"role": "assistant", "content": "hello there"}}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "completion_tokens_details": {"reasoning_tokens": 3}
            }
        });
        let event = provider.inner().parse_response(&response);
        assert_eq!(event.content.as_deref(), Some("hello there"));
        assert_eq!(event.usage.prompt_tokens, 10);
        assert_eq!(event.usage.total_tokens, 15);
        assert_eq!(event.usage.thinking_tokens, 3);
        assert!(event.tool_call.is_none());
    }

    #[test]
    fn response_parsing_handles_tool_calls_and_bad_json() {
        let provider = provider();
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "thinking hard",
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "click", "arguments": "{oops"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        });
        let event = provider.inner().parse_response(&response);
        assert!(event.is_tool_call());
        let tool_call = event.tool_call.clone().unwrap();
        assert_eq!(tool_call.id, "call_9");
        assert_eq!(tool_call.name, "click");
        assert!(tool_call.params.is_empty());
        assert_eq!(
            event.thinking.and_then(|thinking| thinking.content).as_deref(),
            Some("thinking hard")
        );
    }

    #[test]
    fn malformed_response_does_not_panic() {
        let provider = provider();
        let event = provider.inner().parse_response(&json!({}));
        assert_eq!(event.content.as_deref(), Some(""));
        assert_eq!(event.usage, TokenUsage::default());
        let event = provider.inner().parse_response(&json!({"choices": "nope"}));
        assert!(event.tool_call.is_none());
    }

    #[test]
    fn missing_api_key_errors_without_panicking() {
        let provider = ChatOpenAI::new(
            "gpt-4o",
            None,
            Some("https://example.invalid/v1".to_string()),
            600.0,
            2,
            None,
            None,
            None,
            Map::new(),
        );
        // Construction succeeds; the error only appears on use.
        if std::env::var(OPENAI_API_KEY_ENV).is_err() {
            let error = provider.inner().ensure_credentials().unwrap_err();
            assert!(error.to_string().contains(OPENAI_API_KEY_ENV));
        }
    }

    #[test]
    fn metadata_context_windows_match_python_table() {
        let cases = [
            // The four windows called out by the port: gpt-4 / gpt-4-turbo /
            // gpt-3.5 / o1.
            ("gpt-4", 8_192),
            ("gpt-4-turbo", 128_000),
            ("gpt-4-turbo-preview", 128_000),
            ("gpt-3.5-turbo", 16_385),
            ("o1", 200_000),
            ("o1-preview", 200_000),
            // Default branch.
            ("claude-3-5-sonnet", 128_000),
            ("something-new", 128_000),
            // Faithful to Python: `gpt-4o` matches the `gpt-4` prefix first.
            ("gpt-4o", 8_192),
        ];
        for (model, expected) in cases {
            assert_eq!(openai_context_window(model), expected, "model {model}");
        }

        let metadata = ChatOpenAI::with_model("gpt-4").get_metadata();
        assert_eq!(metadata.context_window, 8_192);
        assert_eq!(metadata.owned_by, "openai");
        assert_eq!(metadata.name, "gpt-4");
    }

    #[test]
    fn stream_accumulates_text() {
        let mut state = StreamState::default();
        let mut events = accumulate_chunk(
            &mut state,
            &json!({"choices": [{"delta": {"content": "he"}}]}),
            false,
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, LLMStreamEventType::TextStart);
        assert_eq!(events[1].content.as_deref(), Some("he"));

        events = accumulate_chunk(
            &mut state,
            &json!({"choices": [{"delta": {"content": "llo"}}]}),
            false,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].content.as_deref(), Some("llo"));

        let closing = finish_stream(&mut state);
        assert_eq!(closing.len(), 1);
        assert_eq!(closing[0].event_type, LLMStreamEventType::TextEnd);
    }

    #[test]
    fn stream_emits_thinking_before_text() {
        let mut state = StreamState::default();
        let events = accumulate_chunk(
            &mut state,
            &json!({"choices": [{"delta": {"reasoning_content": "hmm", "content": "answer"}}]}),
            false,
        );
        let kinds: Vec<LLMStreamEventType> = events.iter().map(|event| event.event_type).collect();
        assert_eq!(
            kinds,
            vec![
                LLMStreamEventType::ThinkStart,
                LLMStreamEventType::ThinkDelta,
                LLMStreamEventType::ThinkEnd,
                LLMStreamEventType::TextStart,
                LLMStreamEventType::TextDelta
            ]
        );
    }

    #[test]
    fn stream_finishes_with_tool_call() {
        let mut state = StreamState::default();
        accumulate_chunk(
            &mut state,
            &json!({"choices": [{"delta": {"tool_calls": [
                {"id": "call_1", "function": {"name": "click", "arguments": "{\"x\""}}
            ]}}]}),
            false,
        );
        accumulate_chunk(
            &mut state,
            &json!({"choices": [{"delta": {"tool_calls": [
                {"function": {"arguments": ":1}"}}
            ]}}]}),
            false,
        );
        let closing = finish_stream(&mut state);
        assert_eq!(closing.len(), 1);
        let tool_call = closing[0].tool_call.clone().unwrap();
        assert_eq!(tool_call.id, "call_1");
        assert_eq!(tool_call.name, "click");
        assert_eq!(tool_call.params.get("x"), Some(&json!(1)));
    }

    #[test]
    fn usage_only_chunk_updates_usage() {
        let mut state = StreamState::default();
        let events = accumulate_chunk(
            &mut state,
            &json!({"choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}}),
            false,
        );
        assert!(events.is_empty());
        assert_eq!(state.usage.unwrap().total_tokens, 9);
    }

    #[test]
    fn mistral_tool_call_ids_are_nine_alphanumeric() {
        assert_eq!(mistral_tool_call_id("toolu_01ABCdefGHIjkl"), "toolu01AB");
        assert_eq!(mistral_tool_call_id("abc"), "abc000000");
    }

    #[test]
    fn auth_styles_render_expected_headers() {
        let bearer = AuthStyle::Bearer.headers(Some("k"));
        assert!(bearer.contains(&("Authorization".to_string(), "Bearer k".to_string())));
        let azure = AuthStyle::ApiKeyHeader.headers(Some("k"));
        assert!(azure.contains(&("api-key".to_string(), "k".to_string())));
        let none = AuthStyle::Bearer.headers(None);
        assert!(!none.iter().any(|(name, _)| name == "Authorization"));
    }
}
