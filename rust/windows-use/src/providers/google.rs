//! Google Gemini provider, mirroring `windows_use/providers/google/llm.py`.
//!
//! Native HTTP against `{base}/v1beta/models/{model}:generateContent`, including
//! Gemini's strict user/model alternation (consecutive same-role turns are
//! merged), `functionCall` / `functionResponse` parts, `thought` parts and
//! `thoughtSignature` replay.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::messages::BaseMessage;
use crate::providers::base::{
    json_event_stream, json_i64, require_api_key, resolve_setting, BoxFuture, ChatLlm, EventStream,
    HttpTransport, StreamFraming,
};
use crate::providers::events::{
    LLMEvent, LLMEventType, LLMStreamEvent, LLMStreamEventType, Thinking, ToolCall,
};
use crate::providers::tools::ToolSchema;
use crate::providers::views::{Metadata, TokenUsage};

/// Default base URL for the Gemini API.
pub const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com";
/// Primary environment variable for the Google API key.
pub const GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
/// Fallback environment variable for the Google API key.
pub const GOOGLE_API_KEY_ENV: &str = "GOOGLE_API_KEY";
/// Environment variable overriding the Google base URL.
pub const GOOGLE_BASE_URL_ENV: &str = "GOOGLE_BASE_URL";
/// Upstream default model.
pub const GOOGLE_DEFAULT_MODEL: &str = "gemini-2.5-flash";
/// Default context window for Gemini 2.x models.
pub const GOOGLE_CONTEXT_WINDOW: i64 = 1_048_576;

/// Google Gemini chat model, mirroring `ChatGoogle`.
#[derive(Debug, Clone)]
pub struct ChatGoogle {
    /// Configured model id.
    pub model: String,
    /// Resolved API key.
    pub api_key: Option<String>,
    /// Base URL (no trailing slash).
    pub base_url: String,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Thinking budget for Gemini 2.5 (`-1` dynamic, `0` disabled).
    pub thinking_budget: Option<i64>,
    /// Thinking level for Gemini 3+ (`minimal`/`low`/`medium`/`high`).
    pub thinking_level: Option<String>,
    /// Extra config keys (Python `**kwargs`).
    pub kwargs: Map<String, Value>,
    transport: HttpTransport,
}

impl ChatGoogle {
    /// Mirrors `ChatGoogle.__init__`.
    ///
    /// The key falls back to `GEMINI_API_KEY` then `GOOGLE_API_KEY`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: f64,
        temperature: Option<f64>,
        thinking_budget: Option<i64>,
        thinking_level: Option<String>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let api_key = resolve_setting(
            api_key,
            &[GEMINI_API_KEY_ENV, GOOGLE_API_KEY_ENV],
        );
        let base_url = resolve_setting(base_url, &[GOOGLE_BASE_URL_ENV])
            .unwrap_or_else(|| GOOGLE_BASE_URL.to_string());

        Self {
            model: model.into(),
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            temperature,
            thinking_budget,
            thinking_level,
            kwargs,
            transport: HttpTransport::new(timeout, 2),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, 600.0, None, None, None, Map::new())
    }

    /// `:generateContent` URL.
    pub fn generate_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url, self.model
        )
    }

    /// `:streamGenerateContent` URL with SSE framing.
    pub fn stream_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url, self.model
        )
    }

    /// Validates that a request can be attempted.
    pub fn ensure_credentials(&self) -> Result<()> {
        require_api_key(
            &self.api_key,
            "google",
            &[GEMINI_API_KEY_ENV, GOOGLE_API_KEY_ENV],
        )
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![(
            "Content-Type".to_string(),
            "application/json".to_string(),
        )];
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.is_empty()) {
            headers.push(("x-goog-api-key".to_string(), key.to_string()));
        }
        headers
    }

    /// Converts messages to `(system_instruction, contents)`.
    ///
    /// Consecutive same-role contents are merged because Gemini demands strict
    /// user/model alternation — the same post-processing as
    /// `_merge_consecutive_contents`.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> (Option<String>, Vec<Value>) {
        let mut contents: Vec<Value> = Vec::new();
        let mut system_instruction: Option<String> = None;

        for message in messages {
            match message {
                BaseMessage::SystemMessage { content } => {
                    system_instruction = Some(content.clone());
                }
                BaseMessage::HumanMessage { content } => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": content}],
                    }));
                }
                BaseMessage::ImageMessage { .. } => {
                    let mut parts: Vec<Value> = Vec::new();
                    let text = message.content().unwrap_or_default();
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                    let (payloads, mime_type) = message.convert_images_bytes_with_mime();
                    for payload in payloads {
                        use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
                        use base64::Engine as _;
                        parts.push(json!({
                            "inline_data": {
                                "mime_type": mime_type,
                                "data": BASE64_STANDARD.encode(payload),
                            }
                        }));
                    }
                    contents.push(json!({"role": "user", "parts": parts}));
                }
                BaseMessage::AIMessage {
                    content, thinking, ..
                } => {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(thinking) = thinking {
                        parts.push(json!({"thought": true, "text": thinking}));
                    }
                    if let Some(text) = content.as_deref().filter(|text| !text.is_empty()) {
                        parts.push(json!({"text": text}));
                    }
                    if !parts.is_empty() {
                        contents.push(json!({"role": "model", "parts": parts}));
                    }
                }
                BaseMessage::ToolMessage {
                    name,
                    params,
                    content,
                    thinking,
                    thinking_signature,
                    ..
                } => {
                    let mut model_parts: Vec<Value> = Vec::new();
                    if let Some(thinking) = thinking {
                        model_parts.push(json!({"thought": true, "text": thinking}));
                    }
                    let mut function_call = json!({
                        "function_call": {"name": name, "args": Value::Object(params.clone())},
                    });
                    if let Some(signature) = thinking_signature {
                        if let Value::Object(map) = &mut function_call {
                            map.insert(
                                "thought_signature".to_string(),
                                Value::String(signature.clone()),
                            );
                        }
                    }
                    model_parts.push(function_call);
                    contents.push(json!({"role": "model", "parts": model_parts}));

                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "function_response": {
                                "name": name,
                                "response": {"result": content.clone().unwrap_or_default()},
                            }
                        }],
                    }));
                }
            }
        }

        (system_instruction, merge_consecutive_contents(contents))
    }

    /// Converts tools to Gemini `function_declarations`.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        if tools.is_empty() {
            return Vec::new();
        }
        let declarations: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parametersJsonSchema": tool.parameters_or_empty(),
                })
            })
            .collect();
        vec![json!({"function_declarations": declarations})]
    }

    /// Builds the `generateContent` request body, mirroring `_build_config`.
    pub fn build_request(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Map<String, Value> {
        let (system_instruction, contents) = self.convert_messages(messages);
        let mut body: Map<String, Value> = Map::new();
        body.insert("contents".to_string(), Value::Array(contents));

        let mut generation_config = Map::new();
        for (key, value) in &self.kwargs {
            generation_config.insert(key.clone(), value.clone());
        }

        if let Some(system) = system_instruction {
            body.insert(
                "systemInstruction".to_string(),
                json!({"parts": [{"text": system}]}),
            );
        }

        if !tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(self.convert_tools(tools)));
            body.insert(
                "automaticFunctionCalling".to_string(),
                json!({"disable": true}),
            );
        }

        if let Some(temperature) = self.temperature {
            generation_config.insert("temperature".to_string(), json!(temperature));
        }

        // thinking_budget wins over thinking_level, like the Python branch.
        if let Some(budget) = self.thinking_budget {
            generation_config.insert("thinkingConfig".to_string(), json!({"thinkingBudget": budget}));
        } else if let Some(level) = &self.thinking_level {
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({"thinkingLevel": level}),
            );
        }

        if json_mode {
            generation_config.insert(
                "responseMimeType".to_string(),
                json!("application/json"),
            );
        }

        if !generation_config.is_empty() {
            body.insert(
                "generationConfig".to_string(),
                Value::Object(generation_config),
            );
        }

        body
    }

    /// Parses a `generateContent` reply, mirroring `_process_response`.
    pub fn parse_response(&self, response: &Value) -> LLMEvent {
        let usage = parse_google_usage(response.get("usageMetadata"));
        let parts = response_parts(response);

        for part in &parts {
            if let Some(function_call) = part.get("function_call") {
                let thinking_content = extract_thinking(&parts);
                let signature = extract_signature(&parts);
                let thinking = Thinking::from_parts(thinking_content, signature);
                return LLMEvent::tool_call(ToolCall {
                    id: generated_call_id(),
                    name: function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    params: function_call
                        .get("args")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                })
                .with_thinking(thinking)
                .with_usage(usage);
            }
        }

        let text = extract_text(&parts);
        LLMEvent {
            event_type: LLMEventType::Text,
            thinking: extract_thinking(&parts).map(Thinking::plain),
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
        let body = self.build_request(messages, tools, json_mode);
        let url = self.generate_url();
        let response = self
            .transport
            .post_json(&url, &self.headers(), &Value::Object(body))
            .await
            .with_context(|| format!("google request to {url}"))?;
        Ok(self.parse_response(&response))
    }

    /// Streaming call.
    pub async fn stream_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<EventStream<'static>> {
        self.ensure_credentials()?;
        let body = self.build_request(messages, tools, json_mode);
        let url = self.stream_url();
        let response = self
            .transport
            .post_stream(&url, &self.headers(), &Value::Object(body))
            .await
            .with_context(|| format!("google stream from {url}"))?;

        Ok(json_event_stream(
            response,
            StreamFraming::Sse,
            GoogleStreamState::default(),
            accumulate_google_chunk,
            finish_google_stream,
        ))
    }

    /// Context-window metadata, mirroring `ChatGoogle.get_metadata`.
    pub fn metadata(&self) -> Metadata {
        Metadata {
            name: self.model.clone(),
            context_window: google_context_window(&self.model),
            owned_by: "google".to_string(),
        }
    }
}

/// Gemini's context-window table.
pub fn google_context_window(model: &str) -> i64 {
    if model.contains("1.0") {
        32_768
    } else {
        GOOGLE_CONTEXT_WINDOW
    }
}

/// Collapses consecutive same-role contents by concatenating their parts.
fn merge_consecutive_contents(contents: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(contents.len());
    for content in contents {
        let same_role = match (merged.last(), content.get("role")) {
            (Some(Value::Object(last)), Some(role)) => last.get("role") == Some(role),
            _ => false,
        };
        if same_role {
            let parts = content
                .get("parts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(Value::Object(last)) = merged.last_mut() {
                let mut existing = last
                    .get("parts")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                existing.extend(parts);
                last.insert("parts".to_string(), Value::Array(existing));
            }
        } else {
            merged.push(content);
        }
    }
    merged
}

/// The parts of `candidates[0].content`.
fn response_parts(response: &Value) -> Vec<Value> {
    response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Joins the non-thought text parts.
fn extract_text(parts: &[Value]) -> String {
    let mut text = String::new();
    for part in parts {
        if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            text.push_str(value);
        }
    }
    text
}

/// Joins the thought parts with newlines.
fn extract_thinking(parts: &[Value]) -> Option<String> {
    let thinking: Vec<String> = parts
        .iter()
        .filter(|part| part.get("thought").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if thinking.is_empty() {
        None
    } else {
        Some(thinking.join("\n"))
    }
}

/// The `thought_signature` of the first function-call part.
fn extract_signature(parts: &[Value]) -> Option<String> {
    for part in parts {
        if part.get("function_call").is_some() {
            if let Some(signature) = part.get("thought_signature").and_then(Value::as_str) {
                if !signature.is_empty() {
                    return Some(signature.to_string());
                }
            }
        }
    }
    None
}

/// `call_{uuid.uuid4().hex[:8]}` — Gemini does not return tool-call ids.
pub fn generated_call_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("call_{}", &hex[..8])
}

/// Parses Gemini's `usageMetadata`.
pub fn parse_google_usage(usage: Option<&Value>) -> TokenUsage {
    let Some(usage) = usage else {
        return TokenUsage::default();
    };
    TokenUsage {
        prompt_tokens: json_i64(usage.get("promptTokenCount")).unwrap_or(0),
        completion_tokens: json_i64(usage.get("candidatesTokenCount")).unwrap_or(0),
        total_tokens: json_i64(usage.get("totalTokenCount")).unwrap_or(0),
        thinking_tokens: json_i64(usage.get("thoughtsTokenCount")).unwrap_or(0),
        ..TokenUsage::default()
    }
}

/// Accumulator for streamed `streamGenerateContent` chunks.
#[derive(Debug, Default)]
pub struct GoogleStreamState {
    text_started: bool,
    think_started: bool,
    usage: Option<TokenUsage>,
}

fn accumulate_google_chunk(state: &mut GoogleStreamState, chunk: Value) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();

    if let Some(usage) = chunk.get("usageMetadata") {
        state.usage = Some(parse_google_usage(Some(usage)));
    }

    let parts = response_parts(&chunk);
    for part in &parts {
        // Thinking parts stream before the answer.
        if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    if !state.think_started {
                        state.think_started = true;
                        events.push(LLMStreamEvent::think_start());
                    }
                    events.push(LLMStreamEvent::think_delta(text));
                }
            }
        }

        if let Some(function_call) = part.get("function_call") {
            if state.think_started {
                state.think_started = false;
                events.push(LLMStreamEvent::think_end());
            }
            events.push(
                LLMStreamEvent::new(LLMStreamEventType::ToolCall)
                    .with_tool_call(ToolCall {
                        id: generated_call_id(),
                        name: function_call
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        params: function_call
                            .get("args")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                    })
                    .with_usage(state.usage),
            );
        }
    }

    let text = extract_text(&parts);
    if !text.is_empty() {
        if state.think_started {
            state.think_started = false;
            events.push(LLMStreamEvent::think_end());
        }
        if !state.text_started {
            state.text_started = true;
            events.push(LLMStreamEvent::text_start());
        }
        events.push(LLMStreamEvent::text_delta(text));
    }

    events
}

fn finish_google_stream(state: &mut GoogleStreamState) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();
    if state.think_started {
        state.think_started = false;
        events.push(LLMStreamEvent::think_end());
    }
    if state.text_started {
        state.text_started = false;
        events.push(LLMStreamEvent::text_end(state.usage));
    }
    events
}

impl ChatLlm for ChatGoogle {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> &str {
        "google"
    }

    fn get_metadata(&self) -> Metadata {
        self.metadata()
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

    fn provider() -> ChatGoogle {
        ChatGoogle::new(
            "gemini-2.5-flash",
            Some("k".to_string()),
            Some("https://example.invalid".to_string()),
            600.0,
            None,
            None,
            None,
            Map::new(),
        )
    }

    #[test]
    fn consecutive_same_role_contents_are_merged() {
        let provider = provider();
        let messages = vec![
            BaseMessage::system("be nice"),
            BaseMessage::human("first"),
            BaseMessage::human("second"),
        ];
        let (system, contents) = provider.convert_messages(&messages);
        assert_eq!(system.as_deref(), Some("be nice"));
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "first");
        assert_eq!(parts[1]["text"], "second");
    }

    #[test]
    fn tool_message_produces_function_call_and_response() {
        let provider = provider();
        let messages = vec![BaseMessage::ToolMessage {
            id: "call_1".to_string(),
            name: "click".to_string(),
            params: json!({"x": 1}).as_object().cloned().unwrap(),
            content: Some("ok".to_string()),
            thinking: Some("reason".to_string()),
            thinking_signature: Some("sig".to_string()),
            usage: None,
        }];
        let (_, contents) = provider.convert_messages(&messages);
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "model");
        assert_eq!(contents[0]["parts"][0]["thought"], true);
        assert_eq!(contents[0]["parts"][1]["function_call"]["name"], "click");
        assert_eq!(contents[0]["parts"][1]["thought_signature"], "sig");
        assert_eq!(contents[1]["role"], "user");
        assert_eq!(
            contents[1]["parts"][0]["function_response"]["name"],
            "click"
        );
        assert_eq!(
            contents[1]["parts"][0]["function_response"]["response"]["result"],
            "ok"
        );
    }

    #[test]
    fn function_call_parts_parse_into_tool_call_events() {
        let provider = provider();
        let event = provider.parse_response(&json!({
            "candidates": [{
                "content": {"parts": [
                    {"thought": true, "text": "let me think"},
                    {"function_call": {"name": "click", "args": {"x": 3}}, "thought_signature": "sig"}
                ]}
            }],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 6,
                "totalTokenCount": 11,
                "thoughtsTokenCount": 2
            }
        }));
        assert!(event.is_tool_call());
        let tool_call = event.tool_call.unwrap();
        assert!(tool_call.id.starts_with("call_"));
        assert_eq!(tool_call.name, "click");
        assert_eq!(tool_call.params.get("x"), Some(&json!(3)));
        let thinking = event.thinking.unwrap();
        assert_eq!(thinking.content.as_deref(), Some("let me think"));
        assert_eq!(thinking.signature.as_deref(), Some("sig"));
        assert_eq!(event.usage.prompt_tokens, 5);
        assert_eq!(event.usage.total_tokens, 11);
        assert_eq!(event.usage.thinking_tokens, 2);
    }

    #[test]
    fn text_parts_exclude_thoughts() {
        let provider = provider();
        let event = provider.parse_response(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "hidden"},
                {"text": "visible "},
                {"text": "answer"}
            ]}}],
            "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2, "totalTokenCount": 3}
        }));
        assert_eq!(event.content.as_deref(), Some("visible answer"));
        assert_eq!(
            event.thinking.and_then(|thinking| thinking.content).as_deref(),
            Some("hidden")
        );
    }

    #[test]
    fn context_window_matches_python_table() {
        assert_eq!(google_context_window("gemini-2.5-flash"), 1_048_576);
        assert_eq!(google_context_window("gemini-2.0-flash"), 1_048_576);
        assert_eq!(google_context_window("gemini-1.5-pro"), 1_048_576);
        assert_eq!(google_context_window("gemini-1.0-pro"), 32_768);
        let metadata = provider().get_metadata();
        assert_eq!(metadata.owned_by, "google");
        assert_eq!(metadata.context_window, 1_048_576);
    }

    #[test]
    fn thinking_budget_wins_over_level() {
        let provider = ChatGoogle::new(
            "gemini-2.5-flash",
            Some("k".to_string()),
            None,
            600.0,
            Some(0.3),
            Some(-1),
            Some("high".to_string()),
            Map::new(),
        );
        let body = provider.build_request(&[BaseMessage::human("hi")], &[], false);
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"], -1);
        assert!(body["generationConfig"]["thinkingConfig"]
            .get("thinkingLevel")
            .is_none());
        assert_eq!(body["generationConfig"]["temperature"], 0.3);
    }

    #[test]
    fn tools_use_function_declarations() {
        let provider = provider();
        let tools = vec![ToolSchema::new(
            "click",
            "Click a control",
            json!({"type": "object", "properties": {"x": {"type": "integer"}}}),
        )];
        let converted = provider.convert_tools(&tools);
        assert_eq!(converted[0]["function_declarations"][0]["name"], "click");
        assert_eq!(
            converted[0]["function_declarations"][0]["parametersJsonSchema"]["type"],
            "object"
        );
        let body = provider.build_request(&[BaseMessage::human("hi")], &tools, false);
        assert_eq!(body["automaticFunctionCalling"]["disable"], true);
    }

    #[test]
    fn json_mode_sets_response_mime_type() {
        let provider = provider();
        let body = provider.build_request(&[BaseMessage::human("hi")], &[], true);
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn stream_chunks_emit_thinking_then_text() {
        let mut state = GoogleStreamState::default();
        let mut events = accumulate_google_chunk(
            &mut state,
            json!({"candidates": [{"content": {"parts": [{"thought": true, "text": "hmm"}]}}]}),
        );
        assert_eq!(events[0].event_type, LLMStreamEventType::ThinkStart);
        assert_eq!(events[1].event_type, LLMStreamEventType::ThinkDelta);

        events = accumulate_google_chunk(
            &mut state,
            json!({"candidates": [{"content": {"parts": [{"text": "answer"}]}}],
                   "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2, "totalTokenCount": 3}}),
        );
        let kinds: Vec<LLMStreamEventType> = events.iter().map(|event| event.event_type).collect();
        assert_eq!(
            kinds,
            vec![
                LLMStreamEventType::ThinkEnd,
                LLMStreamEventType::TextStart,
                LLMStreamEventType::TextDelta
            ]
        );

        let closing = finish_google_stream(&mut state);
        assert_eq!(closing[0].event_type, LLMStreamEventType::TextEnd);
        assert_eq!(closing[0].usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn generated_call_ids_look_like_python() {
        let id = generated_call_id();
        assert!(id.starts_with("call_"));
        assert_eq!(id.len(), 13);
    }
}
