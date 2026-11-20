//! Shared provider data models, mirroring `windows_use/providers/views.py`.

use serde::{Deserialize, Serialize};

/// Token usage reported by a provider.
///
/// Every field defaults to `0` (the Python model uses `int | None = 0`); use the
/// `Option`-free fields directly and compare against `0` where Python checks for
/// `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub image_tokens: i64,
    #[serde(default)]
    pub thinking_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
}

impl TokenUsage {
    /// Token usage with the three headline counters filled in.
    pub fn new(prompt_tokens: i64, completion_tokens: i64, total_tokens: i64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            ..Self::default()
        }
    }

    /// Mirrors `TokenUsage(prompt_tokens=..., completion_tokens=..., thinking_tokens=...)`
    /// where every value may be absent.
    pub fn from_counts(
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
        total_tokens: Option<i64>,
        thinking_tokens: Option<i64>,
    ) -> Self {
        Self {
            prompt_tokens: prompt_tokens.unwrap_or(0),
            completion_tokens: completion_tokens.unwrap_or(0),
            total_tokens: total_tokens.unwrap_or(0),
            thinking_tokens: thinking_tokens.unwrap_or(0),
            ..Self::default()
        }
    }

    /// `None` when the provider reported nothing, matching Python's `or None` idiom.
    pub fn thinking_tokens_or_none(&self) -> Option<i64> {
        if self.thinking_tokens == 0 {
            None
        } else {
            Some(self.thinking_tokens)
        }
    }
}

/// Model metadata used for context-window budgeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub context_window: i64,
    pub owned_by: String,
}

impl Metadata {
    pub fn new(name: impl Into<String>, context_window: i64, owned_by: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            context_window,
            owned_by: owned_by.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_defaults_are_zero() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.thinking_tokens_or_none(), None);
    }

    #[test]
    fn metadata_serializes_all_fields() {
        let metadata = Metadata::new("gpt-4", 8192, "openai");
        let value = serde_json::to_value(&metadata).unwrap();
        assert_eq!(value["name"], "gpt-4");
        assert_eq!(value["context_window"], 8192);
        assert_eq!(value["owned_by"], "openai");
    }
}
