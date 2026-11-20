//! Perplexity provider, mirroring `windows_use/providers/perplexity/llm.py`.
//!
//! Perplexity's Agent API (`POST /v1/agent`) speaks the Responses format, not
//! Chat Completions: system prompts become `instructions`, tool calls become flat
//! `function_call` / `function_call_output` items, and tool schemas use the flat
//! `{type, name, description, parameters}` shape.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::messages::BaseMessage;
use crate::providers::base::{
    decode_json_response, json_i64, resolve_setting, single_event_stream, BoxFuture, ChatLlm,
    EventStream, HttpTransport,
};
use crate::providers::events::{LLMEvent, LLMEventType};
use crate::providers::tools::{parse_tool_arguments, sanitize_schema, serialize_tool_arguments, ToolSchema};
use crate::providers::views::{Metadata, TokenUsage};

/// Default base URL for the Perplexity API.
pub const PERPLEXITY_BASE_URL: &str = "https://api.perplexity.ai";
/// Environment variable holding the Perplexity API key.
pub const PERPLEXITY_API_KEY_ENV: &str = "PERPLEXITY_API_KEY";
/// Upstream default model.
pub const PERPLEXITY_DEFAULT_MODEL: &str = "sonar-pro";
/// Context window reported by `ChatPerplexity.get_metadata`.
pub const PERPLEXITY_CONTEXT_WINDOW: i64 = 127_072;
/// Placeholder key used when the environment provides none.
pub const PERPLEXITY_MISSING_KEY: &str = "missing_key";

/// A known Perplexity model, mirroring `perplexity/view.py`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerplexityModelInfo {
    /// Name used on the wire.
    pub api_name: String,
    /// Whether the model accepts images.
    pub vision: bool,
    /// Whether the model supports a reasoning effort.
    pub reasoning_support: bool,
    /// Reasoning effort requested when supported.
    pub reasoning_effort: Option<String>,
}

/// Model registry, mirroring `MODEL_MAPPINGS` plus the pass-through fallback.
pub fn get_model_info(model_id: &str) -> PerplexityModelInfo {
    let known: &[(&str, bool, bool)] = &[
        ("perplexity/sonar", false, false),
        ("openai/gpt-5.4", true, true),
        ("google/gemini-3.1-pro-preview", true, true),
        ("google/gemini-3-flash-preview", true, true),
        ("anthropic/claude-sonnet-4-6", true, true),
        ("anthropic/claude-opus-4-6", true, true),
    ];
    match known.iter().find(|(name, _, _)| *name == model_id) {
        Some((name, vision, reasoning)) => PerplexityModelInfo {
            api_name: (*name).to_string(),
            vision: *vision,
            reasoning_support: *reasoning,
            reasoning_effort: if *reasoning {
                Some("low".to_string())
            } else {
                None
            },
        },
        None => PerplexityModelInfo {
            api_name: model_id.to_string(),
            vision: true,
            reasoning_support: false,
            reasoning_effort: None,
        },
    }
}

/// Perplexity chat model, mirroring `ChatPerplexity`.
#[derive(Debug, Clone)]
pub struct ChatPerplexity {
    /// The registry model id the caller asked for.
    pub model: String,
    /// The model name actually sent to the API.
    pub api_model: String,
    /// Resolved API key (`missing_key` when absent, like Python).
    pub api_key: String,
    /// Base URL (no trailing slash).
    pub base_url: String,
    /// Sampling temperature (Python default `0.5`).
    pub temperature: f64,
    /// Output token ceiling (Python default `16000`).
    pub max_output_tokens: i64,
    /// Registry information for the model.
    pub model_info: PerplexityModelInfo,
    /// Extra request-body keys (Python's `**kwargs`).
    pub extra_kwargs: Map<String, Value>,
    transport: HttpTransport,
}

impl ChatPerplexity {
    /// Mirrors `ChatPerplexity.__init__` (temperature `0.5`, 16000 output tokens).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        temperature: Option<f64>,
        max_output_tokens: Option<i64>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let model = model.into();
        let model_info = get_model_info(&model);
        let api_key = resolve_setting(api_key, &[PERPLEXITY_API_KEY_ENV])
            .unwrap_or_else(|| PERPLEXITY_MISSING_KEY.to_string());
        let base_url = base_url
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| PERPLEXITY_BASE_URL.to_string());

        let mut provider = Self {
            api_model: model_info.api_name.clone(),
            model,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            temperature: temperature.unwrap_or(0.5),
            max_output_tokens: max_output_tokens.unwrap_or(16_000),
            model_info,
            extra_kwargs: kwargs,
            transport: HttpTransport::new(timeout, 2),
        };
        provider.extra_kwargs.retain(|key, _| !key.is_empty());
        provider
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, 600.0, None, None, Map::new())
    }

    /// The `/v1/agent` URL.
    pub fn agent_url(&self) -> String {
        format!("{}/v1/agent", self.base_url)
    }

    /// Perplexity reports a missing key the same way Python does: it warns and
    /// uses a placeholder so construction never fails.
    pub fn has_api_key(&self) -> bool {
        self.api_key != PERPLEXITY_MISSING_KEY && !self.api_key.is_empty()
    }

    fn headers(&self) -> Vec<(String, String)> {
        vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    /// Converts messages to `(instructions, input)`, mirroring `_convert_messages`.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> (Option<String>, Vec<Value>) {
        let mut instructions: Option<String> = None;
        let mut input: Vec<Value> = Vec::with_capacity(messages.len());

        for message in messages {
            match message {
                BaseMessage::SystemMessage { content } => {
                    instructions = Some(content.clone());
                }
                BaseMessage::ImageMessage { .. } => {
                    let mut parts: Vec<Value> = Vec::new();
                    let text = message.content().unwrap_or_default();
                    if !text.is_empty() {
                        parts.push(json!({"type": "input_text", "text": text}));
                    }
                    let (encoded, mime_type) = message.convert_images_base64_with_mime();
                    for payload in encoded {
                        parts.push(json!({
                            "type": "input_image",
                            "image_url": format!("data:{mime_type};base64,{payload}"),
                        }));
                    }
                    input.push(json!({"role": "user", "content": parts}));
                }
                BaseMessage::HumanMessage { content } => {
                    input.push(json!({
                        "role": "user",
                        "content": [{"type": "input_text", "text": content}],
                    }));
                }
                BaseMessage::AIMessage { content, .. } => {
                    let text = content.clone().unwrap_or_default();
                    input.push(json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                BaseMessage::ToolMessage {
                    id,
                    name,
                    params,
                    content,
                    ..
                } => {
                    // The Responses format needs the call and its result as items.
                    input.push(json!({
                        "type": "function_call",
                        "name": name,
                        "arguments": serialize_tool_arguments(params),
                        "call_id": id,
                    }));
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": id,
                        "output": content.clone().unwrap_or_default(),
                    }));
                }
            }
        }

        (instructions, input)
    }

    /// Converts tools to the flat Responses shape, sanitizing each schema.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        tools
            .iter()
            .map(|tool| {
                let sanitized = sanitize_schema(&tool.to_json_schema());
                json!({
                    "type": "function",
                    "name": sanitized.get("name").cloned().unwrap_or(Value::Null),
                    "description": sanitized.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": sanitized.get("parameters").cloned().unwrap_or(Value::Null),
                })
            })
            .collect()
    }

    /// Builds the request body, mirroring `_build_request`.
    pub fn build_request(&self, messages: &[BaseMessage], tools: &[ToolSchema]) -> Map<String, Value> {
        let (instructions, input) = self.convert_messages(messages);

        let mut data: Map<String, Value> = Map::new();
        for (key, value) in &self.extra_kwargs {
            data.insert(key.clone(), value.clone());
        }
        data.insert("model".to_string(), json!(self.api_model));
        data.insert("input".to_string(), Value::Array(input));
        data.insert("max_output_tokens".to_string(), json!(self.max_output_tokens));
        data.insert("temperature".to_string(), json!(self.temperature));

        if let Some(instructions) = instructions {
            data.insert("instructions".to_string(), json!(instructions));
        }

        if !tools.is_empty() {
            data.insert("tools".to_string(), Value::Array(self.convert_tools(tools)));
        }

        if self.model_info.reasoning_support {
            data.insert(
                "reasoning".to_string(),
                json!({"effort": self
                    .model_info
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "low".to_string())}),
            );
        }

        data
    }

    /// Parses the Agent API reply, mirroring `_parse_response`.
    pub fn parse_response(&self, result: &Value) -> LLMEvent {
        let usage_data = result.get("usage");
        let input_tokens = usage_data
            .and_then(|usage| json_i64(usage.get("input_tokens")))
            .unwrap_or(0);
        let output_tokens = usage_data
            .and_then(|usage| json_i64(usage.get("output_tokens")))
            .unwrap_or(0);
        let usage = TokenUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
            ..TokenUsage::default()
        };

        let items = result
            .get("output")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for item in &items {
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                return LLMEvent::tool_call(crate::providers::events::ToolCall {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    params: parse_tool_arguments(item.get("arguments")),
                })
                .with_usage(usage);
            }

            if item.get("type").and_then(Value::as_str) == Some("message") {
                if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("output_text") {
                            return LLMEvent {
                                event_type: LLMEventType::Text,
                                thinking: None,
                                content: Some(
                                    block
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                tool_call: None,
                                usage,
                            };
                        }
                    }
                }
            }
        }

        LLMEvent {
            event_type: LLMEventType::Text,
            thinking: None,
            content: Some(String::new()),
            tool_call: None,
            usage,
        }
    }

    /// Citations returned by the Sonar models, when the response carries any.
    pub fn extract_citations(&self, result: &Value) -> Vec<String> {
        let mut citations = Vec::new();
        if let Some(items) = result.get("citations").and_then(Value::as_array) {
            for item in items {
                if let Some(url) = item.as_str() {
                    citations.push(url.to_string());
                } else if let Some(url) = item.get("url").and_then(Value::as_str) {
                    citations.push(url.to_string());
                }
            }
        }
        // Agent API surfaces search results under `output` items.
        if let Some(items) = result.get("output").and_then(Value::as_array) {
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("message") {
                    if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                        for block in blocks {
                            if let Some(annotations) =
                                block.get("annotations").and_then(Value::as_array)
                            {
                                for annotation in annotations {
                                    if let Some(url) =
                                        annotation.get("url").and_then(Value::as_str)
                                    {
                                        citations.push(url.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        citations
    }

    /// Non-streaming call.
    pub async fn invoke_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
    ) -> Result<LLMEvent> {
        if !self.has_api_key() {
            anyhow::bail!(
                "perplexity API key is not set; pass it to the constructor or set {PERPLEXITY_API_KEY_ENV}"
            );
        }
        let body = self.build_request(messages, tools);
        let url = self.agent_url();
        let response = self
            .transport
            .client()
            .post(&url)
            .headers(reqwest_headers(&self.headers()))
            .json(&Value::Object(body))
            .send()
            .await
            .with_context(|| format!("perplexity request to {url}"))?;
        let value = decode_json_response(response).await?;
        Ok(self.parse_response(&value))
    }
}

/// Builds a `reqwest` header map from string pairs.
fn reqwest_headers(headers: &[(String, String)]) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            map.insert(name, value);
        }
    }
    map
}

impl ChatLlm for ChatPerplexity {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> &str {
        "perplexity"
    }

    fn get_metadata(&self) -> Metadata {
        Metadata {
            name: self.model.clone(),
            context_window: PERPLEXITY_CONTEXT_WINDOW,
            owned_by: "perplexity".to_string(),
        }
    }

    fn invoke<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<LLMEvent>> {
        Box::pin(async move { self.invoke_messages(messages, tools).await })
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<EventStream<'a>>> {
        // The Agent API has no streaming; emit the buffered result as a stream.
        Box::pin(async move {
            let event = self.invoke_messages(messages, tools).await?;
            Ok(single_event_stream(event))
        })
    }
}
