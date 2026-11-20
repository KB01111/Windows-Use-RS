//! Mistral AI provider, mirroring `windows_use/providers/mistral/llm.py`.
//!
//! Mistral is OpenAI-compatible with two twists: tool-call ids must be exactly
//! nine alphanumeric characters (`_mistral_tool_call_id`) and Magistral thinking
//! models pass reasoning as `{"type": "thinking"}` content chunks.

use serde_json::{Map, Value};

use crate::providers::openai::{
    mistral_tool_call_id, openai_compatible_newtype, parse_usage, AuthStyle, CompatConfig,
    CompatQuirks, ContextWindowRule, OpenAiCompatible, ReasoningModelRule,
};
use crate::providers::events::{LLMEvent, LLMEventType, Thinking, ToolCall};
use crate::providers::tools::parse_tool_arguments;
use crate::providers::views::TokenUsage;

/// Default base URL for the Mistral API.
pub const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
/// Environment variable holding the Mistral API key.
pub const MISTRAL_API_KEY_ENV: &str = "MISTRAL_API_KEY";
/// Upstream default model.
pub const MISTRAL_DEFAULT_MODEL: &str = "mistral-large-latest";

/// The configuration of the Mistral provider.
pub fn config() -> CompatConfig {
    CompatConfig {
        provider: "mistral",
        // Mistral serves chat under /chat/completions like OpenAI.
        completions_path: "/chat/completions",
        default_base_url: MISTRAL_BASE_URL,
        base_url_env: &[],
        api_key_env: &[MISTRAL_API_KEY_ENV],
        auth: AuthStyle::Bearer,
        quirks: CompatQuirks {
            mistral_content_chunks: true,
            reasoning_as_reasoning_content: false,
            reasoning_model_rule: ReasoningModelRule::Never,
            ..CompatQuirks::default()
        },
        reasoning_needles: &[],
        context_window: ContextWindowRule::MistralFamily,
        fixed_context_window: 128_000,
        sends_reasoning_effort: false,
    }
}

openai_compatible_newtype!(
    /// Mistral chat model, mirroring `ChatMistral`.
    ChatMistral
);

impl ChatMistral {
    /// Mirrors `ChatMistral.__init__`; `timeout` is optional in Python, `None`
    /// here means "use the default".
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        timeout: Option<f64>,
        temperature: Option<f64>,
        kwargs: Map<String, Value>,
    ) -> Self {
        Self {
            inner: OpenAiCompatible::new(
                config(),
                model,
                api_key,
                base_url,
                timeout.unwrap_or(600.0),
                2,
                None,
                temperature,
                None,
                kwargs,
            ),
        }
    }

    /// Constructor with the Python defaults.
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None, None, None, None, Map::new())
    }

    /// `ChatMistral._extract_content`: normalises Magistral content chunks into
    /// `(text, thinking)`.
    pub fn extract_content(raw_content: Option<&Value>) -> (String, Option<String>) {
        match raw_content {
            Some(Value::String(text)) => (text.clone(), None),
            Some(Value::Array(chunks)) => {
                let mut text_parts = Vec::new();
                let mut thinking_parts = Vec::new();
                for chunk in chunks {
                    match chunk.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = chunk.get("text").and_then(Value::as_str) {
                                text_parts.push(text.to_string());
                            }
                        }
                        Some("thinking") => {
                            if let Some(inner) = chunk.get("thinking").and_then(Value::as_array) {
                                for part in inner {
                                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                                        thinking_parts.push(text.to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let thinking = if thinking_parts.is_empty() {
                    None
                } else {
                    Some(thinking_parts.concat())
                };
                (text_parts.concat(), thinking)
            }
            Some(Value::Null) | None => (String::new(), None),
            Some(other) => (other.to_string(), None),
        }
    }

    /// `ChatMistral._process_response`: like the shared parser but reads thinking
    /// out of the content chunks and estimates thinking tokens when absent.
    pub fn parse_mistral_response(&self, response: &Value) -> LLMEvent {
        let choice = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .cloned()
            .unwrap_or(Value::Null);
        let message = choice.get("message").cloned().unwrap_or(Value::Null);
        let mut usage = parse_usage(response.get("usage"));

        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            if let Some(tool_call) = tool_calls.first() {
                // Thinking may live in the content chunks of a tool-calling turn.
                let (_, thinking) = Self::extract_content(message.get("content"));
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
                .with_thinking(thinking.map(Thinking::plain))
                .with_usage(usage);
            }
        }

        let (text, thinking) = Self::extract_content(message.get("content"));
        // Estimate thinking tokens when the API does not report them.
        if thinking.is_some() && usage.thinking_tokens == 0 {
            usage = TokenUsage {
                thinking_tokens: thinking
                    .as_ref()
                    .map(|thinking| (thinking.chars().count() / 4).max(1) as i64)
                    .unwrap_or(0),
                ..usage
            };
        }

        LLMEvent {
            event_type: LLMEventType::Text,
            thinking: thinking.map(Thinking::plain),
            content: Some(text),
            tool_call: None,
            usage,
        }
    }

    /// `_mistral_tool_call_id`, re-exported for callers converting history.
    pub fn tool_call_id(raw: &str) -> String {
        mistral_tool_call_id(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_call_ids_are_normalised() {
        assert_eq!(ChatMistral::tool_call_id("toolu_01ABCdefGHIjkl"), "toolu01AB");
    }

    #[test]
    fn content_chunks_are_flattened() {
        let (text, thinking) = ChatMistral::extract_content(Some(&json!([
            {"type": "thinking", "thinking": [{"type": "text", "text": "reason"}]},
            {"type": "text", "text": "answer"}
        ])));
        assert_eq!(text, "answer");
        assert_eq!(thinking.as_deref(), Some("reason"));
    }

    #[test]
    fn plain_string_content_is_returned_as_is() {
        let (text, thinking) = ChatMistral::extract_content(Some(&json!("hello")));
        assert_eq!(text, "hello");
        assert!(thinking.is_none());
    }

    #[test]
    fn thinking_tokens_are_estimated_when_missing() {
        let provider = ChatMistral::with_model("magistral-medium-latest");
        let event = provider.parse_mistral_response(&json!({
            "choices": [{"message": {"content": [
                {"type": "thinking", "thinking": [{"type": "text", "text": "abcdefgh"}]},
                {"type": "text", "text": "ok"}
            ]}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }));
        assert_eq!(event.content.as_deref(), Some("ok"));
        assert!(event.usage.thinking_tokens >= 1);
    }

    #[test]
    fn tool_calls_use_normalised_ids() {
        let provider = ChatMistral::with_model("mistral-large-latest");
        let converted = provider.inner().convert_messages(&[
            crate::messages::BaseMessage::ToolMessage {
                id: "toolu_01ABCdefGHIjkl".to_string(),
                name: "click".to_string(),
                params: json!({"x": 1}).as_object().cloned().unwrap(),
                content: Some("ok".to_string()),
                thinking: None,
                thinking_signature: None,
                usage: None,
            },
        ]);
        assert_eq!(converted[0]["tool_calls"][0]["id"], "toolu01AB");
        assert_eq!(converted[1]["tool_call_id"], "toolu01AB");
        assert_eq!(converted[1]["name"], "click");
    }
}
