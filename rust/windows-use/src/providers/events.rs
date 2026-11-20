//! Event models, mirroring `windows_use/providers/events.py`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::views::TokenUsage;

/// High-level event kinds returned by `invoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LLMEventType {
    Text,
    ToolCall,
    Error,
}

impl LLMEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            LLMEventType::Text => "text",
            LLMEventType::ToolCall => "tool_call",
            LLMEventType::Error => "error",
        }
    }
}

/// Incremental event kinds emitted by a streaming call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LLMStreamEventType {
    TextStart,
    TextDelta,
    TextEnd,
    ToolCall,
    ThinkStart,
    ThinkDelta,
    ThinkEnd,
    Error,
}

impl LLMStreamEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            LLMStreamEventType::TextStart => "text_start",
            LLMStreamEventType::TextDelta => "text_delta",
            LLMStreamEventType::TextEnd => "text_end",
            LLMStreamEventType::ToolCall => "tool_call",
            LLMStreamEventType::ThinkStart => "think_start",
            LLMStreamEventType::ThinkDelta => "think_delta",
            LLMStreamEventType::ThinkEnd => "think_end",
            LLMStreamEventType::Error => "error",
        }
    }
}

/// Thinking/reasoning content with an optional cryptographic signature
/// (Anthropic and Gemini attach one so the block can be replayed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thinking {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl Thinking {
    pub fn new(content: Option<String>, signature: Option<String>) -> Self {
        Self { content, signature }
    }

    /// Builds a `Thinking` only when something is present, like the Python
    /// `Thinking(...) if thinking_parts or thinking_signature else None` idiom.
    pub fn from_parts(content: Option<String>, signature: Option<String>) -> Option<Self> {
        if content.is_none() && signature.is_none() {
            None
        } else {
            Some(Self { content, signature })
        }
    }

    /// Convenience constructor for providers that never sign their thinking.
    pub fn plain(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            signature: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.content.as_deref().unwrap_or_default().is_empty() && self.signature.is_none()
    }
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, params: Map<String, Value>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            params,
        }
    }
}

/// Result of a non-streaming call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMEvent {
    #[serde(rename = "type")]
    pub event_type: LLMEventType,
    #[serde(default)]
    pub thinking: Option<Thinking>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_call: Option<ToolCall>,
    #[serde(default)]
    pub usage: TokenUsage,
}

impl LLMEvent {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            event_type: LLMEventType::Text,
            thinking: None,
            content: Some(content.into()),
            tool_call: None,
            usage: TokenUsage::default(),
        }
    }

    pub fn tool_call(tool_call: ToolCall) -> Self {
        Self {
            event_type: LLMEventType::ToolCall,
            thinking: None,
            content: None,
            tool_call: Some(tool_call),
            usage: TokenUsage::default(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            event_type: LLMEventType::Error,
            thinking: None,
            content: Some(message.into()),
            tool_call: None,
            usage: TokenUsage::default(),
        }
    }

    pub fn with_thinking(mut self, thinking: Option<Thinking>) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = usage;
        self
    }

    pub fn is_tool_call(&self) -> bool {
        self.event_type == LLMEventType::ToolCall
    }
}

/// One incremental event from a streaming call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMStreamEvent {
    #[serde(rename = "type")]
    pub event_type: LLMStreamEventType,
    #[serde(default)]
    pub thinking: Option<Thinking>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_call: Option<ToolCall>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

impl LLMStreamEvent {
    pub fn new(event_type: LLMStreamEventType) -> Self {
        Self {
            event_type,
            thinking: None,
            content: None,
            tool_call: None,
            usage: None,
        }
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn with_tool_call(mut self, tool_call: ToolCall) -> Self {
        self.tool_call = Some(tool_call);
        self
    }

    pub fn with_thinking(mut self, thinking: Option<Thinking>) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_usage(mut self, usage: Option<TokenUsage>) -> Self {
        self.usage = usage;
        self
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.TEXT_START)`.
    pub fn text_start() -> Self {
        Self::new(LLMStreamEventType::TextStart)
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.TEXT_DELTA, content=...)`.
    pub fn text_delta(content: impl Into<String>) -> Self {
        Self::new(LLMStreamEventType::TextDelta).with_content(content)
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.THINK_START)`.
    pub fn think_start() -> Self {
        Self::new(LLMStreamEventType::ThinkStart)
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.THINK_DELTA, content=...)`.
    pub fn think_delta(content: impl Into<String>) -> Self {
        Self::new(LLMStreamEventType::ThinkDelta).with_content(content)
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.THINK_END)`.
    pub fn think_end() -> Self {
        Self::new(LLMStreamEventType::ThinkEnd)
    }

    /// `LLMStreamEvent(type=LLMStreamEventType.TEXT_END, usage=...)`.
    pub fn text_end(usage: Option<TokenUsage>) -> Self {
        Self::new(LLMStreamEventType::TextEnd).with_usage(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_wire_values_match_python() {
        assert_eq!(LLMEventType::Text.as_str(), "text");
        assert_eq!(LLMEventType::ToolCall.as_str(), "tool_call");
        assert_eq!(LLMEventType::Error.as_str(), "error");
        assert_eq!(LLMStreamEventType::TextStart.as_str(), "text_start");
        assert_eq!(LLMStreamEventType::TextDelta.as_str(), "text_delta");
        assert_eq!(LLMStreamEventType::TextEnd.as_str(), "text_end");
        assert_eq!(LLMStreamEventType::ToolCall.as_str(), "tool_call");
        assert_eq!(LLMStreamEventType::ThinkStart.as_str(), "think_start");
        assert_eq!(LLMStreamEventType::ThinkDelta.as_str(), "think_delta");
        assert_eq!(LLMStreamEventType::ThinkEnd.as_str(), "think_end");
        assert_eq!(LLMStreamEventType::Error.as_str(), "error");
    }

    #[test]
    fn event_serializes_type_field() {
        let event = LLMEvent::text("hi");
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "text");
        assert_eq!(value["content"], "hi");
        assert_eq!(value["usage"]["total_tokens"], 0);

        let stream = LLMStreamEvent::think_start();
        let value = serde_json::to_value(&stream).unwrap();
        assert_eq!(value["type"], "think_start");
    }

    #[test]
    fn thinking_from_parts_is_none_when_empty() {
        assert!(Thinking::from_parts(None, None).is_none());
        assert!(Thinking::from_parts(Some(String::new()), Some("sig".into())).is_some());
    }
}
