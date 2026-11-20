//! Anthropic provider, mirroring `windows_use/providers/anthropic/llm.py`.
//!
//! Native HTTP against `/v1/messages`: thinking blocks with signatures, prompt
//! caching breakpoints on the system prompt / tools / recent user turns, and the
//! `tool_use` + `tool_result` pairing Anthropic requires.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::messages::BaseMessage;
use crate::providers::base::{
    json_event_stream, json_i64, require_api_key, resolve_setting, BoxFuture, ChatLlm, EventStream,
    HttpTransport, StreamFraming,
};
use crate::providers::events::{LLMEvent, LLMEventType, LLMStreamEvent, LLMStreamEventType, Thinking, ToolCall};
use crate::providers::tools::{parse_tool_arguments, ToolSchema};
use crate::providers::views::{Metadata, TokenUsage};

/// Default base URL for the Anthropic API.
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
/// Environment variable holding the Anthropic API key.
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Environment variable overriding the Anthropic base URL.
pub const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Upstream default model.
pub const ANTHROPIC_DEFAULT_MODEL: &str = "claude-3-5-sonnet-latest";
/// Default context window (`ChatAnthropic.get_metadata`).
pub const ANTHROPIC_CONTEXT_WINDOW: i64 = 200_000;
/// Smallest thinking budget Anthropic accepts.
pub const ANTHROPIC_MIN_THINKING_BUDGET: i64 = 1024;

/// Anthropic chat model, mirroring `ChatAnthropic`.
#[derive(Debug, Clone)]
pub struct ChatAnthropic {
    /// Configured model id.
    pub model: String,
    /// Resolved API key.
    pub api_key: Option<String>,
    /// Base URL (no trailing slash).
    pub base_url: String,
    /// Sampling temperature (ignored when thinking is enabled).
    pub temperature: Option<f64>,
    /// Extended-thinking budget in tokens.
    pub thinking_budget: Option<i64>,
    /// Maximum output tokens.
    pub max_tokens: i64,
    /// Whether prompt caching is enabled at all.
    pub enable_prompt_caching: bool,
    /// Whether the system prompt gets a cache breakpoint.
    pub cache_system_prompt: bool,
    /// Whether the tool list gets a cache breakpoint.
    pub cache_tools: bool,
    /// How many recent user turns get a cache breakpoint.
    pub cache_recent_messages: usize,
    /// Extra request-body keys.
    pub kwargs: Map<String, Value>,
    transport: HttpTransport,
}

impl ChatAnthropic {
    /// Mirrors `ChatAnthropic.__init__`, including the max_tokens adjustment.
    ///
    /// A thinking budget below 1024 is raised to 1024; when `max_tokens` is not
    /// larger than the budget it becomes `budget + 4096`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        max_retries: u32,
        temperature: Option<f64>,
        thinking_budget: Option<i64>,
        max_tokens: i64,
        enable_prompt_caching: bool,
        cache_system_prompt: bool,
        cache_tools: bool,
        cache_recent_messages: usize,
        kwargs: Map<String, Value>,
    ) -> Self {
        let api_key = resolve_setting(api_key, &[ANTHROPIC_API_KEY_ENV]);
        let base_url = resolve_setting(base_url, &[ANTHROPIC_BASE_URL_ENV])
            .unwrap_or_else(|| ANTHROPIC_BASE_URL.to_string());

        let mut thinking_budget = thinking_budget;
        let mut max_tokens = max_tokens;
        if let Some(budget) = thinking_budget {
            let budget = budget.max(ANTHROPIC_MIN_THINKING_BUDGET);
            thinking_budget = Some(budget);
            if max_tokens <= budget {
                max_tokens = budget + 4096;
            }
        }

        Self {
            model: model.into(),
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            temperature,
            thinking_budget,
            max_tokens,
            enable_prompt_caching,
            cache_system_prompt,
            cache_tools,
            cache_recent_messages,
            kwargs,
            transport: HttpTransport::new(timeout, max_retries),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(
            model,
            None,
            None,
            600.0,
            2,
            None,
            None,
            4096,
            true,
            true,
            true,
            2,
            Map::new(),
        )
    }

    /// The `/v1/messages` URL.
    pub fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    /// The thinking flag, mirroring `thinking_budget is not None`.
    pub fn thinking_enabled(&self) -> bool {
        self.thinking_budget.is_some()
    }

    /// Validates that a request can be attempted.
    pub fn ensure_credentials(&self) -> Result<()> {
        require_api_key(&self.api_key, "anthropic", &[ANTHROPIC_API_KEY_ENV])
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
        ];
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.is_empty()) {
            headers.push(("x-api-key".to_string(), key.to_string()));
        }
        headers
    }

    /// Converts messages to `(system_prompt, messages)`, mirroring
    /// `ChatAnthropic._convert_messages`.
    ///
    /// Tool messages become an assistant `tool_use` block appended to the
    /// preceding assistant turn (or their own turn) plus a user `tool_result`.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> (Option<Value>, Vec<Value>) {
        let mut converted: Vec<Value> = Vec::new();
        let mut system_prompt: Option<String> = None;

        for message in messages {
            match message {
                BaseMessage::SystemMessage { content } => {
                    system_prompt = Some(content.clone());
                }
                BaseMessage::HumanMessage { content } => {
                    converted.push(json!({"role": "user", "content": content}));
                }
                BaseMessage::ImageMessage { .. } => {
                    let mut content: Vec<Value> = Vec::new();
                    let text = message.content().unwrap_or_default();
                    if !text.is_empty() {
                        content.push(json!({"type": "text", "text": text}));
                    }
                    let (encoded, mime_type) = message.convert_images_base64_with_mime();
                    for payload in encoded {
                        content.push(json!({
                            "type": "image",
                            "source": {"type": "base64", "media_type": mime_type, "data": payload},
                        }));
                    }
                    converted.push(json!({"role": "user", "content": content}));
                }
                BaseMessage::AIMessage {
                    content,
                    thinking,
                    thinking_signature,
                    ..
                } => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(thinking) = thinking {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": thinking_signature.clone().unwrap_or_default(),
                        }));
                    }
                    if let Some(text) = content.as_deref().filter(|text| !text.is_empty()) {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                    let content = if blocks.is_empty() {
                        Value::String(String::new())
                    } else {
                        Value::Array(blocks)
                    };
                    converted.push(json!({"role": "assistant", "content": content}));
                }
                BaseMessage::ToolMessage {
                    id,
                    name,
                    params,
                    content,
                    thinking,
                    thinking_signature,
                    ..
                } => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(thinking) = thinking {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": thinking_signature.clone().unwrap_or_default(),
                        }));
                    }
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": Value::Object(params.clone()),
                    }));

                    // Anthropic requires tool_use directly before tool_result, so
                    // append onto the preceding assistant turn when there is one.
                    match converted.last_mut() {
                        Some(Value::Object(last))
                            if last.get("role").and_then(Value::as_str) == Some("assistant") =>
                        {
                            let existing = last.remove("content").unwrap_or(Value::Null);
                            let mut merged = match existing {
                                Value::String(text) if !text.is_empty() => {
                                    vec![json!({"type": "text", "text": text})]
                                }
                                Value::Array(blocks) => blocks,
                                _ => Vec::new(),
                            };
                            merged.extend(blocks);
                            last.insert("content".to_string(), Value::Array(merged));
                        }
                        _ => {
                            converted.push(json!({"role": "assistant", "content": blocks}));
                        }
                    }

                    converted.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": content.clone().unwrap_or_default(),
                        }],
                    }));
                }
            }
        }

        // Prompt caching on the system prompt.
        let system = match system_prompt {
            Some(text) if self.enable_prompt_caching && self.cache_system_prompt => {
                Some(json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"},
                }]))
            }
            Some(text) => Some(Value::String(text)),
            None => None,
        };

        // Prompt caching on the last N user turns.
        if self.enable_prompt_caching && self.cache_recent_messages > 0 && !converted.is_empty() {
            let user_indices: Vec<usize> = converted
                .iter()
                .enumerate()
                .filter(|(_, message)| {
                    message.get("role").and_then(Value::as_str) == Some("user")
                })
                .map(|(index, _)| index)
                .collect();
            let start = user_indices.len().saturating_sub(self.cache_recent_messages);
            for index in &user_indices[start..] {
                let entry = &mut converted[*index];
                let existing = entry.get("content").cloned().unwrap_or(Value::Null);
                let mut blocks = match existing {
                    Value::String(text) => vec![json!({"type": "text", "text": text})],
                    Value::Array(blocks) => blocks,
                    other => vec![other],
                };
                if let Some(Value::Object(map)) = blocks.last_mut() {
                    map.insert(
                        "cache_control".to_string(),
                        json!({"type": "ephemeral"}),
                    );
                }
                if let Value::Object(map) = entry {
                    map.insert("content".to_string(), Value::Array(blocks));
                }
            }
        }

        (system, converted)
    }

    /// Converts tools to Anthropic's `input_schema` shape, caching the last one.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        let mut definitions: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters_or_empty(),
                })
            })
            .collect();

        if self.enable_prompt_caching && self.cache_tools {
            if let Some(Value::Object(map)) = definitions.last_mut() {
                map.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
            }
        }

        definitions
    }

    /// Builds the request body, mirroring `_build_params`.
    pub fn build_params(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Map<String, Value> {
        let (system, converted) = self.convert_messages(messages);
        let mut params: Map<String, Value> = Map::new();

        // Python spreads kwargs into the body.
        for (key, value) in &self.kwargs {
            params.insert(key.clone(), value.clone());
        }

        params.insert("model".to_string(), json!(self.model));
        params.insert("messages".to_string(), Value::Array(converted));
        params.insert("max_tokens".to_string(), json!(self.max_tokens));

        if let Some(system) = system {
            params.insert("system".to_string(), system);
        }

        if !tools.is_empty() {
            params.insert("tools".to_string(), Value::Array(self.convert_tools(tools)));
        }

        if let Some(budget) = self.thinking_budget {
            // Extended thinking forbids a custom temperature.
            params.insert(
                "thinking".to_string(),
                json!({"type": "enabled", "budget_tokens": budget}),
            );
        } else if let Some(temperature) = self.temperature {
            params.insert("temperature".to_string(), json!(temperature));
        }

        if json_mode {
            // Anthropic has no JSON mode; nudge the model in the system prompt.
            let nudge = "Respond with a single valid JSON object and nothing else.";
            let merged = match params.get("system") {
                Some(Value::String(text)) => Value::String(format!("{text}\n\n{nudge}")),
                Some(Value::Array(blocks)) => {
                    let mut blocks = blocks.clone();
                    blocks.push(json!({"type": "text", "text": nudge}));
                    Value::Array(blocks)
                }
                _ => Value::String(nudge.to_string()),
            };
            params.insert("system".to_string(), merged);
        }

        params
    }

    /// Parses a `/v1/messages` reply, mirroring `_process_response`.
    pub fn parse_response(&self, response: &Value) -> LLMEvent {
        let usage = parse_anthropic_usage(response.get("usage"));
        let blocks = response
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut text = String::new();
        let mut thinking_parts: Vec<String> = Vec::new();
        let mut thinking_signature: Option<String> = None;

        for block in &blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(value) = block.get("text").and_then(Value::as_str) {
                        text.push_str(value);
                    }
                }
                Some("thinking") => {
                    if let Some(value) = block.get("thinking").and_then(Value::as_str) {
                        thinking_parts.push(value.to_string());
                    }
                    if let Some(signature) = block.get("signature").and_then(Value::as_str) {
                        if !signature.is_empty() {
                            thinking_signature = Some(signature.to_string());
                        }
                    }
                }
                Some("tool_use") => {
                    let thinking = thinking_object(&thinking_parts, &thinking_signature);
                    let params = parse_tool_arguments(block.get("input"));
                    return LLMEvent::tool_call(ToolCall {
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        params,
                    })
                    .with_thinking(thinking)
                    .with_usage(usage);
                }
                _ => {}
            }
        }

        LLMEvent {
            event_type: LLMEventType::Text,
            thinking: thinking_object(&thinking_parts, &thinking_signature),
            content: Some(text),
            tool_call: None,
            usage,
        }
    }

    /// Non-streaming call.
    pub async fn invoke_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<LLMEvent> {
        self.ensure_credentials()?;
        let params = self.build_params(messages, tools, json_mode);
        let url = self.messages_url();
        let response = self
            .transport
            .post_json(&url, &self.headers(), &Value::Object(params))
            .await
            .with_context(|| format!("anthropic request to {url}"))?;
        Ok(self.parse_response(&response))
    }

    /// Streaming call, decoding Anthropic's SSE event types.
    pub async fn stream_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<EventStream<'static>> {
        self.ensure_credentials()?;
        let mut params = self.build_params(messages, tools, json_mode);
        params.insert("stream".to_string(), json!(true));
        let url = self.messages_url();
        let response = self
            .transport
            .post_stream(&url, &self.headers(), &Value::Object(params))
            .await
            .with_context(|| format!("anthropic stream from {url}"))?;

        Ok(json_event_stream(
            response,
            StreamFraming::Sse,
            AnthropicStreamState::default(),
            accumulate_anthropic_event,
            finish_anthropic_stream,
        ))
    }

    /// Cache configuration, mirroring `get_cache_stats`.
    pub fn get_cache_stats(&self) -> Value {
        json!({
            "enabled": self.enable_prompt_caching,
            "cache_system_prompt": self.cache_system_prompt,
            "cache_tools": self.cache_tools,
            "cache_recent_messages": self.cache_recent_messages,
            "model": self.model,
        })
    }
}

/// Builds the `Thinking` object from collected blocks, matching Python's
/// "only when something is present" rule.
fn thinking_object(parts: &[String], signature: &Option<String>) -> Option<Thinking> {
    let content = if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    };
    Thinking::from_parts(content, signature.clone())
}

/// Parses Anthropic's `usage` object, including cache counters.
fn parse_anthropic_usage(usage: Option<&Value>) -> TokenUsage {
    let Some(usage) = usage else {
        return TokenUsage::default();
    };
    let prompt_tokens = json_i64(usage.get("input_tokens")).unwrap_or(0);
    let completion_tokens = json_i64(usage.get("output_tokens")).unwrap_or(0);
    TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        thinking_tokens: json_i64(usage.get("thinking_tokens")).unwrap_or(0),
        cache_creation_input_tokens: json_i64(usage.get("cache_creation_input_tokens"))
            .unwrap_or(0),
        cache_read_input_tokens: json_i64(usage.get("cache_read_input_tokens")).unwrap_or(0),
        ..TokenUsage::default()
    }
}

/// Accumulator for Anthropic streaming events.
#[derive(Debug, Default)]
pub struct AnthropicStreamState {
    text_started: bool,
    think_started: bool,
    usage: Option<TokenUsage>,
    tool_call_id: Option<String>,
    tool_call_name: Option<String>,
    tool_call_args: String,
    thinking_parts: Vec<String>,
    thinking_signature: Option<String>,
    /// Index of the content block currently being streamed.
    current_block: Option<i64>,
    block_types: std::collections::HashMap<i64, String>,
}

fn accumulate_anthropic_event(
    state: &mut AnthropicStreamState,
    event: Value,
) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default();

    match event_type {
        "message_start" => {
            if let Some(usage) = event.get("message").and_then(|message| message.get("usage")) {
                state.usage = Some(parse_anthropic_usage(Some(usage)));
            }
        }
        "content_block_start" => {
            let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
            state.current_block = Some(index);
            if let Some(block) = event.get("content_block") {
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or_default();
                state
                    .block_types
                    .insert(index, block_type.to_string());
                if block_type == "tool_use" {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        state.tool_call_id = Some(id.to_string());
                    }
                    if let Some(name) = block.get("name").and_then(Value::as_str) {
                        state.tool_call_name = Some(name.to_string());
                    }
                }
                if block_type == "thinking" && !state.think_started {
                    state.think_started = true;
                    events.push(LLMStreamEvent::think_start());
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = event.get("delta") {
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            if state.think_started {
                                state.think_started = false;
                                events.push(LLMStreamEvent::think_end());
                            }
                            if !state.text_started {
                                state.text_started = true;
                                events.push(LLMStreamEvent::text_start());
                            }
                            if !text.is_empty() {
                                events.push(LLMStreamEvent::text_delta(text));
                            }
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            state.thinking_parts.push(text.to_string());
                            if !state.think_started {
                                state.think_started = true;
                                events.push(LLMStreamEvent::think_start());
                            }
                            events.push(LLMStreamEvent::think_delta(text));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            state.thinking_signature = Some(signature.to_string());
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            state.tool_call_args.push_str(partial);
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            state.current_block = None;
        }
        "message_delta" => {
            if let Some(usage) = event.get("usage") {
                let mut merged = state.usage.unwrap_or_default();
                if let Some(output) = json_i64(usage.get("output_tokens")) {
                    merged.completion_tokens = output;
                }
                if let Some(input) = json_i64(usage.get("input_tokens")) {
                    merged.prompt_tokens = input;
                }
                merged.total_tokens = merged.prompt_tokens + merged.completion_tokens;
                if let Some(thinking) = json_i64(usage.get("thinking_tokens")) {
                    merged.thinking_tokens = thinking;
                }
                state.usage = Some(merged);
            }
        }
        "error" => {
            events.push(
                LLMStreamEvent::new(LLMStreamEventType::Error).with_content(
                    event
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("anthropic stream error"),
                ),
            );
        }
        _ => {}
    }

    events
}

fn finish_anthropic_stream(state: &mut AnthropicStreamState) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();
    if state.think_started {
        state.think_started = false;
        events.push(LLMStreamEvent::think_end());
    }

    let thinking = thinking_object(&state.thinking_parts, &state.thinking_signature);

    if let (Some(id), Some(name)) = (state.tool_call_id.clone(), state.tool_call_name.clone()) {
        let params = crate::providers::tools::parse_tool_arguments_str(&state.tool_call_args);
        events.push(
            LLMStreamEvent::new(LLMStreamEventType::ToolCall)
                .with_tool_call(ToolCall { id, name, params })
                .with_thinking(thinking)
                .with_usage(state.usage),
        );
    } else if state.text_started {
        state.text_started = false;
        events.push(
            LLMStreamEvent::new(LLMStreamEventType::TextEnd)
                .with_thinking(thinking)
                .with_usage(state.usage),
        );
    }

    events
}

impl ChatLlm for ChatAnthropic {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> &str {
        "anthropic"
    }

    fn get_metadata(&self) -> Metadata {
        Metadata {
            name: self.model.clone(),
            context_window: ANTHROPIC_CONTEXT_WINDOW,
            owned_by: "anthropic".to_string(),
        }
    }

    fn invoke<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<LLMEvent>> {
        Box::pin(async move { self.invoke_messages(messages, tools, false).await })
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<EventStream<'a>>> {
        Box::pin(async move { self.stream_messages(messages, tools, false).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Usage;

    fn provider() -> ChatAnthropic {
        ChatAnthropic::new(
            "claude-3-5-sonnet-latest",
            Some("test-key".to_string()),
            Some("https://example.invalid".to_string()),
            600.0,
            2,
            None,
            None,
            4096,
            true,
            true,
            true,
            2,
            Map::new(),
        )
    }

    #[test]
    fn message_conversion_matches_anthropic_wire_format() {
        let provider = provider();
        let messages = vec![
            BaseMessage::system("be terse"),
            BaseMessage::human("hello"),
            BaseMessage::AIMessage {
                content: Some("hi".to_string()),
                thinking: Some("pondering".to_string()),
                thinking_signature: Some("sig-abc".to_string()),
                usage: Some(Usage::new(1, 2, 3)),
            },
        ];
        let (system, converted) = provider.convert_messages(&messages);

        // System prompts become the top-level `system` field, cached.
        let system = system.unwrap();
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["text"], "be terse");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");

        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "user");
        // Human content becomes a list only once caching is applied.
        assert_eq!(converted[0]["content"][0]["text"], "hello");
        assert_eq!(converted[0]["content"][0]["cache_control"]["type"], "ephemeral");

        assert_eq!(converted[1]["role"], "assistant");
        assert_eq!(converted[1]["content"][0]["type"], "thinking");
        assert_eq!(converted[1]["content"][0]["thinking"], "pondering");
        assert_eq!(converted[1]["content"][0]["signature"], "sig-abc");
        assert_eq!(converted[1]["content"][1]["type"], "text");
        assert_eq!(converted[1]["content"][1]["text"], "hi");
    }

    #[test]
    fn tool_messages_produce_tool_use_and_tool_result() {
        let provider = ChatAnthropic::new(
            "claude-3-5-sonnet-latest",
            Some("k".to_string()),
            None,
            600.0,
            2,
            None,
            None,
            4096,
            false,
            false,
            false,
            0,
            Map::new(),
        );
        let messages = vec![
            BaseMessage::human("click the button"),
            BaseMessage::ToolMessage {
                id: "toolu_1".to_string(),
                name: "click".to_string(),
                params: serde_json::json!({"x": 5}).as_object().cloned().unwrap(),
                content: Some("clicked".to_string()),
                thinking: None,
                thinking_signature: None,
                usage: None,
            },
        ];
        let (system, converted) = provider.convert_messages(&messages);
        assert!(system.is_none());
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[1]["role"], "assistant");
        assert_eq!(converted[1]["content"][0]["type"], "tool_use");
        assert_eq!(converted[1]["content"][0]["id"], "toolu_1");
        assert_eq!(converted[1]["content"][0]["input"]["x"], 5);
        assert_eq!(converted[2]["role"], "user");
        assert_eq!(converted[2]["content"][0]["type"], "tool_result");
        assert_eq!(converted[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(converted[2]["content"][0]["content"], "clicked");
    }

    #[test]
    fn thinking_budget_adjusts_max_tokens() {
        // Budget below 1024 is raised; a max_tokens that no longer exceeds the
        // budget becomes `budget + 4096`.
        let provider = ChatAnthropic::new(
            "claude-3-7-sonnet-latest",
            Some("k".to_string()),
            None,
            600.0,
            2,
            Some(0.9),
            Some(500),
            1000,
            false,
            false,
            false,
            0,
            Map::new(),
        );
        assert_eq!(provider.thinking_budget, Some(1024));
        assert_eq!(provider.max_tokens, 5120);
        assert!(provider.thinking_enabled());

        let params = provider.build_params(&[BaseMessage::human("hi")], &[], false);
        assert_eq!(params["thinking"]["budget_tokens"], 1024);
        // Temperature is not sent when thinking is enabled.
        assert!(!params.contains_key("temperature"));
    }

    #[test]
    fn max_tokens_is_kept_when_already_above_the_budget() {
        // Python only grows max_tokens when it is <= the budget.
        let provider = ChatAnthropic::new(
            "claude-3-7-sonnet-latest",
            Some("k".to_string()),
            None,
            600.0,
            2,
            None,
            Some(500),
            4096,
            false,
            false,
            false,
            0,
            Map::new(),
        );
        assert_eq!(provider.thinking_budget, Some(1024));
        assert_eq!(provider.max_tokens, 4096);
    }

    #[test]
    fn temperature_is_sent_without_thinking() {
        let provider = ChatAnthropic::new(
            "claude-3-5-sonnet-latest",
            Some("k".to_string()),
            None,
            600.0,
            2,
            Some(0.2),
            None,
            4096,
            false,
            false,
            false,
            0,
            Map::new(),
        );
        let params = provider.build_params(&[BaseMessage::human("hi")], &[], false);
        assert_eq!(params["temperature"], 0.2);
        assert!(!params.contains_key("thinking"));
    }

    #[test]
    fn response_parsing_collects_text_thinking_and_usage() {
        let provider = provider();
        let event = provider.parse_response(&serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "let me think", "signature": "sig"},
                {"type": "text", "text": "the answer"}
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_creation_input_tokens": 5,
                "cache_read_input_tokens": 7
            }
        }));
        assert_eq!(event.content.as_deref(), Some("the answer"));
        assert_eq!(event.usage.prompt_tokens, 10);
        assert_eq!(event.usage.completion_tokens, 20);
        assert_eq!(event.usage.total_tokens, 30);
        assert_eq!(event.usage.cache_creation_input_tokens, 5);
        assert_eq!(event.usage.cache_read_input_tokens, 7);
        let thinking = event.thinking.unwrap();
        assert_eq!(thinking.content.as_deref(), Some("let me think"));
        assert_eq!(thinking.signature.as_deref(), Some("sig"));
    }

    #[test]
    fn response_parsing_picks_the_first_tool_use() {
        let provider = provider();
        let event = provider.parse_response(&serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "s"},
                {"type": "tool_use", "id": "toolu_9", "name": "click", "input": {"x": 1}}
            ],
            "usage": {"input_tokens": 3, "output_tokens": 4}
        }));
        assert!(event.is_tool_call());
        let tool_call = event.tool_call.unwrap();
        assert_eq!(tool_call.id, "toolu_9");
        assert_eq!(tool_call.name, "click");
        assert_eq!(tool_call.params.get("x"), Some(&serde_json::json!(1)));
        assert_eq!(event.usage.total_tokens, 7);
    }

    #[test]
    fn invalid_tool_input_yields_empty_params() {
        let provider = provider();
        let event = provider.parse_response(&serde_json::json!({
            "content": [{"type": "tool_use", "id": "t", "name": "n", "input": "not-an-object"}],
            "usage": {}
        }));
        assert!(event.tool_call.unwrap().params.is_empty());
    }

    #[test]
    fn stream_events_map_to_thinking_and_text() {
        let mut state = AnthropicStreamState::default();
        let mut events = accumulate_anthropic_event(
            &mut state,
            serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking"}}),
        );
        assert_eq!(events[0].event_type, LLMStreamEventType::ThinkStart);

        events = accumulate_anthropic_event(
            &mut state,
            serde_json::json!({
                "type": "content_block_delta",
                "delta": {"type": "thinking_delta", "thinking": "hmm"}
            }),
        );
        assert_eq!(events[0].event_type, LLMStreamEventType::ThinkDelta);

        events = accumulate_anthropic_event(
            &mut state,
            serde_json::json!({
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": "answer"}
            }),
        );
        let kinds: Vec<LLMStreamEventType> = events.iter().map(|event| event.event_type).collect();
        assert_eq!(
            kinds,
            vec![LLMStreamEventType::ThinkEnd, LLMStreamEventType::TextStart, LLMStreamEventType::TextDelta]
        );

        let closing = finish_anthropic_stream(&mut state);
        assert_eq!(closing[0].event_type, LLMStreamEventType::TextEnd);
    }

    #[test]
    fn streamed_tool_use_is_assembled_from_json_deltas() {
        let mut state = AnthropicStreamState::default();
        accumulate_anthropic_event(
            &mut state,
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "click"}
            }),
        );
        accumulate_anthropic_event(
            &mut state,
            serde_json::json!({
                "type": "content_block_delta",
                "delta": {"type": "input_json_delta", "partial_json": "{\"x\": 1}"}
            }),
        );
        let closing = finish_anthropic_stream(&mut state);
        let tool_call = closing[0].tool_call.clone().unwrap();
        assert_eq!(tool_call.id, "toolu_1");
        assert_eq!(tool_call.params.get("x"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn metadata_is_200k_context() {
        let metadata = provider().get_metadata();
        assert_eq!(metadata.context_window, 200_000);
        assert_eq!(metadata.owned_by, "anthropic");
        assert_eq!(provider().provider(), "anthropic");
    }

    #[test]
    fn tools_use_input_schema_and_cache_the_last() {
        let provider = provider();
        let tools = vec![
            ToolSchema::new("a", "first", serde_json::json!({"type": "object"})),
            ToolSchema::new("b", "second", serde_json::json!({"type": "object"})),
        ];
        let converted = provider.convert_tools(&tools);
        assert_eq!(converted[0]["name"], "a");
        assert_eq!(converted[0]["input_schema"]["type"], "object");
        assert!(converted[0].get("cache_control").is_none());
        assert_eq!(converted[1]["cache_control"]["type"], "ephemeral");
    }
}
