//! Ollama provider, mirroring `windows_use/providers/ollama/llm.py`.
//!
//! Native HTTP against `POST {host}/api/chat` with newline-delimited JSON when
//! streaming. Images travel as a bare base64 array, tool calls come back without
//! ids (so one is generated) and thinking is estimated at 4 chars/token.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::messages::BaseMessage;
use crate::providers::base::{
    json_event_stream, json_i64, BoxFuture, ChatLlm, EventStream, HttpTransport, StreamFraming,
};
use crate::providers::events::{
    LLMEvent, LLMEventType, LLMStreamEvent, LLMStreamEventType, Thinking, ToolCall,
};
use crate::providers::tools::{parse_tool_arguments, ToolSchema};
use crate::providers::views::{Metadata, TokenUsage};

/// Default Ollama host.
pub const OLLAMA_HOST: &str = "http://localhost:11434";
/// Environment variable overriding the host.
pub const OLLAMA_HOST_ENV: &str = "OLLAMA_HOST";
/// Upstream default model.
pub const OLLAMA_DEFAULT_MODEL: &str = "llama3.1";
/// Context window assumed when the server reports none.
pub const OLLAMA_CONTEXT_WINDOW: i64 = 32_768;

/// Ollama chat model, mirroring `ChatOllama`.
#[derive(Debug, Clone)]
pub struct ChatOllama {
    /// Configured model id.
    pub model: String,
    /// Resolved host URL (no trailing slash).
    pub host: String,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Thinking flag: `true`/`false` or an effort level (`low`/`medium`/`high`).
    pub think: Option<Value>,
    /// Extra request-body keys.
    pub kwargs: Map<String, Value>,
    transport: HttpTransport,
}

impl ChatOllama {
    /// Mirrors `ChatOllama.__init__`; the host falls back to `OLLAMA_HOST`
    /// then `http://localhost:11434`.
    pub fn new(
        model: impl Into<String>,
        host: Option<String>,
        timeout: f64,
        temperature: Option<f64>,
        think: Option<Value>,
        kwargs: Map<String, Value>,
    ) -> Self {
        let host = host
            .filter(|host| !host.is_empty())
            .or_else(|| crate::providers::base::env_var(OLLAMA_HOST_ENV))
            .unwrap_or_else(|| OLLAMA_HOST.to_string());

        Self {
            model: model.into(),
            host: host.trim_end_matches('/').to_string(),
            temperature,
            think,
            kwargs,
            transport: HttpTransport::new(timeout, 0),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, 600.0, None, None, Map::new())
    }

    /// The `/api/chat` URL.
    pub fn chat_url(&self) -> String {
        format!("{}/api/chat", self.host)
    }

    /// Ollama needs no credentials; kept for symmetry with the other providers.
    pub fn ensure_credentials(&self) -> Result<()> {
        Ok(())
    }

    /// Converts messages to Ollama's wire format.
    ///
    /// Images ride along as a bare base64 array and tool calls use
    /// `{function: {name, arguments}}` with no ids.
    pub fn convert_messages(&self, messages: &[BaseMessage]) -> Vec<Value> {
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
                    let encoded = message.convert_images_base64();
                    converted.push(json!({
                        "role": "user",
                        "content": message.content().unwrap_or_default(),
                        "images": encoded,
                    }));
                }
                BaseMessage::AIMessage {
                    content, thinking, ..
                } => {
                    let mut entry = Map::new();
                    entry.insert("role".to_string(), json!("assistant"));
                    entry.insert(
                        "content".to_string(),
                        json!(content.clone().unwrap_or_default()),
                    );
                    if let Some(thinking) = thinking {
                        entry.insert("thinking".to_string(), json!(thinking));
                    }
                    converted.push(Value::Object(entry));
                }
                BaseMessage::ToolMessage {
                    name,
                    params,
                    content,
                    thinking,
                    ..
                } => {
                    let mut assistant = Map::new();
                    assistant.insert("role".to_string(), json!("assistant"));
                    assistant.insert("content".to_string(), json!(""));
                    assistant.insert(
                        "tool_calls".to_string(),
                        json!([{
                            "function": {
                                "name": name,
                                "arguments": Value::Object(params.clone()),
                            }
                        }]),
                    );
                    if let Some(thinking) = thinking {
                        assistant.insert("thinking".to_string(), json!(thinking));
                    }
                    converted.push(Value::Object(assistant));
                    converted.push(json!({
                        "role": "tool",
                        "content": content.clone().unwrap_or_default(),
                    }));
                }
            }
        }

        converted
    }

    /// Converts tools to Ollama's OpenAI-style function wrapper.
    pub fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        tools.iter().map(ToolSchema::to_openai_tool).collect()
    }

    /// Builds the `/api/chat` body.
    pub fn build_params(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
        stream: bool,
    ) -> Map<String, Value> {
        let mut params: Map<String, Value> = Map::new();
        for (key, value) in &self.kwargs {
            params.insert(key.clone(), value.clone());
        }

        params.insert("model".to_string(), json!(self.model));
        params.insert("messages".to_string(), json!(self.convert_messages(messages)));

        if !tools.is_empty() {
            params.insert("tools".to_string(), json!(self.convert_tools(tools)));
        }

        if let Some(think) = &self.think {
            params.insert("think".to_string(), think.clone());
        }

        if stream {
            params.insert("stream".to_string(), json!(true));
        }

        if let Some(temperature) = self.temperature {
            let mut options = params
                .get("options")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            options.insert("temperature".to_string(), json!(temperature));
            params.insert("options".to_string(), Value::Object(options));
        }

        if json_mode {
            params.insert("format".to_string(), json!("json"));
        }

        params
    }

    /// Parses an `/api/chat` reply, mirroring `_process_response`.
    pub fn parse_response(&self, response: &Value) -> LLMEvent {
        let message = response.get("message").cloned().unwrap_or(Value::Null);
        let thinking = message
            .get("thinking")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string);

        let prompt_tokens = json_i64(response.get("prompt_eval_count")).unwrap_or(0);
        let completion_tokens = json_i64(response.get("eval_count")).unwrap_or(0);
        // Ollama exposes no thinking token count; estimate at 4 chars/token.
        let thinking_tokens = thinking
            .as_ref()
            .map(|text| ((text.chars().count() / 4) as i64).max(1))
            .unwrap_or(0);

        let usage = TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            thinking_tokens,
            ..TokenUsage::default()
        };

        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            if let Some(tool_call) = tool_calls.first() {
                let function = tool_call.get("function").cloned().unwrap_or(Value::Null);
                return LLMEvent::tool_call(ToolCall {
                    id: generated_call_id(),
                    name: function
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    params: parse_tool_arguments(function.get("arguments")),
                })
                .with_thinking(thinking.map(Thinking::plain))
                .with_usage(usage);
            }
        }

        LLMEvent {
            event_type: LLMEventType::Text,
            thinking: thinking.map(Thinking::plain),
            content: Some(
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
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
        let params = self.build_params(messages, tools, json_mode, false);
        let url = self.chat_url();
        let response = self
            .transport
            .post_json(&url, &[], &Value::Object(params))
            .await
            .with_context(|| format!("ollama request to {url}"))?;
        Ok(self.parse_response(&response))
    }

    /// Streaming call over newline-delimited JSON.
    pub async fn stream_messages(
        &self,
        messages: &[BaseMessage],
        tools: &[ToolSchema],
        json_mode: bool,
    ) -> Result<EventStream<'static>> {
        let params = self.build_params(messages, tools, json_mode, true);
        let url = self.chat_url();
        let response = self
            .transport
            .post_stream(&url, &[], &Value::Object(params))
            .await
            .with_context(|| format!("ollama stream from {url}"))?;

        Ok(json_event_stream(
            response,
            StreamFraming::Lines,
            OllamaStreamState::default(),
            accumulate_ollama_chunk,
            finish_ollama_stream,
        ))
    }
}

/// `call_{uuid.uuid4().hex[:8]}` — Ollama does not return tool-call ids.
pub fn generated_call_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("call_{}", &hex[..8])
}

/// Accumulator for streamed `/api/chat` lines.
#[derive(Debug, Default)]
pub struct OllamaStreamState {
    text_started: bool,
    think_started: bool,
    thinking: Vec<String>,
    usage: Option<TokenUsage>,
}

fn accumulate_ollama_chunk(state: &mut OllamaStreamState, chunk: Value) -> Vec<LLMStreamEvent> {
    let mut events = Vec::new();
    let message = chunk.get("message").cloned().unwrap_or(Value::Null);

    // Usage may arrive on any line; Ollama reports it on the final one.
    if chunk.get("eval_count").is_some() || chunk.get("prompt_eval_count").is_some() {
        let prompt_tokens = json_i64(chunk.get("prompt_eval_count")).unwrap_or(0);
        let completion_tokens = json_i64(chunk.get("eval_count")).unwrap_or(0);
        let thinking_tokens = if state.thinking.is_empty() {
            0
        } else {
            let chars: usize = state.thinking.iter().map(|part| part.chars().count()).sum();
            ((chars / 4) as i64).max(1)
        };
        state.usage = Some(TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            thinking_tokens,
            ..TokenUsage::default()
        });
    }

    if let Some(thinking) = message
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        state.thinking.push(thinking.to_string());
        if !state.think_started {
            state.think_started = true;
            events.push(LLMStreamEvent::think_start());
        }
        events.push(LLMStreamEvent::think_delta(thinking));
    }

    if let Some(content) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        if state.think_started {
            state.think_started = false;
            events.push(LLMStreamEvent::think_end());
        }
        if !state.text_started {
            state.text_started = true;
            events.push(LLMStreamEvent::text_start());
        }
        events.push(LLMStreamEvent::text_delta(content));
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        if let Some(tool_call) = tool_calls.first() {
            let function = tool_call.get("function").cloned().unwrap_or(Value::Null);
            if state.think_started {
                state.think_started = false;
                events.push(LLMStreamEvent::think_end());
            }
            let thinking = if state.thinking.is_empty() {
                None
            } else {
                Some(Thinking::plain(state.thinking.concat()))
            };
            events.push(
                LLMStreamEvent::new(LLMStreamEventType::ToolCall)
                    .with_tool_call(ToolCall {
                        id: generated_call_id(),
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        params: parse_tool_arguments(function.get("arguments")),
                    })
                    .with_thinking(thinking)
                    .with_usage(state.usage),
            );
        }
    }

    events
}

fn finish_ollama_stream(state: &mut OllamaStreamState) -> Vec<LLMStreamEvent> {
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

impl ChatLlm for ChatOllama {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> &str {
        "ollama"
    }

    fn get_metadata(&self) -> Metadata {
        Metadata {
            name: self.model.clone(),
            context_window: OLLAMA_CONTEXT_WINDOW,
            owned_by: "ollama".to_string(),
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

    fn provider() -> ChatOllama {
        ChatOllama::new(
            "llama3.1",
            Some("http://localhost:11434".to_string()),
            600.0,
            None,
            None,
            Map::new(),
        )
    }

    #[test]
    fn host_defaults_and_trims() {
        let provider = ChatOllama::new(
            "m",
            Some("http://box:11434/".to_string()),
            600.0,
            None,
            None,
            Map::new(),
        );
        assert_eq!(provider.chat_url(), "http://box:11434/api/chat");
        assert_eq!(provider.get_metadata().context_window, 32_768);
        assert_eq!(provider.provider(), "ollama");
    }

    #[test]
    fn tool_messages_reconstruct_history() {
        let provider = provider();
        let converted = provider.convert_messages(&[BaseMessage::ToolMessage {
            id: "call_1".to_string(),
            name: "click".to_string(),
            params: json!({"x": 1}).as_object().cloned().unwrap(),
            content: Some("ok".to_string()),
            thinking: Some("why".to_string()),
            thinking_signature: None,
            usage: None,
        }]);
        assert_eq!(converted[0]["role"], "assistant");
        assert_eq!(
            converted[0]["tool_calls"][0]["function"]["name"],
            "click"
        );
        assert_eq!(converted[0]["tool_calls"][0]["function"]["arguments"]["x"], 1);
        assert_eq!(converted[0]["thinking"], "why");
        // Ollama tool results carry no tool_call_id.
        assert_eq!(converted[1]["role"], "tool");
        assert_eq!(converted[1]["content"], "ok");
        assert!(converted[1].get("tool_call_id").is_none());
    }

    #[test]
    fn temperature_goes_into_options() {
        let provider = ChatOllama::new("m", None, 600.0, Some(0.4), None, Map::new());
        let params = provider.build_params(&[BaseMessage::human("hi")], &[], false, false);
        assert_eq!(params["options"]["temperature"], 0.4);
    }

    #[test]
    fn thinking_and_stream_flags_are_forwarded() {
        let provider = ChatOllama::new(
            "m",
            None,
            600.0,
            None,
            Some(json!("high")),
            Map::new(),
        );
        let params = provider.build_params(&[BaseMessage::human("hi")], &[], false, true);
        assert_eq!(params["think"], "high");
        assert_eq!(params["stream"], true);
    }

    #[test]
    fn response_parsing_generates_tool_call_ids() {
        let provider = provider();
        let event = provider.parse_response(&json!({
            "message": {
                "content": "",
                "thinking": "thinking hard",
                "tool_calls": [{"function": {"name": "click", "arguments": {"x": 2}}}]
            },
            "prompt_eval_count": 4,
            "eval_count": 6
        }));
        assert!(event.is_tool_call());
        let tool_call = event.tool_call.unwrap();
        assert!(tool_call.id.starts_with("call_"));
        assert_eq!(tool_call.name, "click");
        assert_eq!(tool_call.params.get("x"), Some(&json!(2)));
        assert_eq!(event.usage.prompt_tokens, 4);
        assert_eq!(event.usage.total_tokens, 10);
        // ~4 chars per token for "thinking hard" (13 chars) => at least 1.
        assert!(event.usage.thinking_tokens >= 1);
    }

    #[test]
    fn string_arguments_are_decoded() {
        let provider = provider();
        let event = provider.parse_response(&json!({
            "message": {"tool_calls": [{"function": {"name": "click", "arguments": "{\"x\": 9}"}}]}
        }));
        assert_eq!(event.tool_call.unwrap().params.get("x"), Some(&json!(9)));
    }

    #[test]
    fn invalid_string_arguments_yield_empty_params() {
        let provider = provider();
        let event = provider.parse_response(&json!({
            "message": {"tool_calls": [{"function": {"name": "click", "arguments": "{bad"}}]}
        }));
        assert!(event.tool_call.unwrap().params.is_empty());
    }

    #[test]
    fn text_response_without_usage_is_safe() {
        let provider = provider();
        let event = provider.parse_response(&json!({"message": {"content": "hi"}}));
        assert_eq!(event.content.as_deref(), Some("hi"));
        assert_eq!(event.usage, TokenUsage::default());
        assert!(event.thinking.is_none());
    }

    #[test]
    fn stream_lines_emit_thinking_then_text() {
        let mut state = OllamaStreamState::default();
        let mut events = accumulate_ollama_chunk(
            &mut state,
            json!({"message": {"thinking": "hmm"}}),
        );
        assert_eq!(events[0].event_type, LLMStreamEventType::ThinkStart);

        events = accumulate_ollama_chunk(&mut state, json!({"message": {"content": "answer"}}));
        let kinds: Vec<LLMStreamEventType> = events.iter().map(|event| event.event_type).collect();
        assert_eq!(
            kinds,
            vec![
                LLMStreamEventType::ThinkEnd,
                LLMStreamEventType::TextStart,
                LLMStreamEventType::TextDelta
            ]
        );

        let closing = finish_ollama_stream(&mut state);
        assert_eq!(closing[0].event_type, LLMStreamEventType::TextEnd);
    }

    #[test]
    fn stream_usage_is_read_from_the_final_line() {
        let mut state = OllamaStreamState::default();
        accumulate_ollama_chunk(&mut state, json!({"message": {"content": "hi"}}));
        accumulate_ollama_chunk(
            &mut state,
            json!({"message": {"content": ""}, "prompt_eval_count": 3, "eval_count": 4}),
        );
        let closing = finish_ollama_stream(&mut state);
        let usage = closing[0].usage.unwrap();
        assert_eq!(usage.prompt_tokens, 3);
        assert_eq!(usage.total_tokens, 7);
    }
}
