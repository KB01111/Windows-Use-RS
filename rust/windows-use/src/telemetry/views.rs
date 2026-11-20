//! Telemetry event payloads, mirroring `windows_use/telemetry/views.py`.
//!
//! The Python module defines a `BaseTelemetryEvent` dataclass whose `properties`
//! are `dataclasses.asdict(self)`, and an `AgentTelemetryEvent` carrying the run
//! summary. Here the base becomes a trait and the concrete event keeps exactly
//! the same JSON field names, including `event_name`, because `asdict` includes
//! it in the payload.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::VERSION;

/// The event name sent for every agent run (`AgentTelemetryEvent.event_name`).
pub const AGENT_EVENT_NAME: &str = "agent_event";

/// Anything that can be captured by [`crate::telemetry::ProductTelemetry`].
///
/// Mirrors the Python `BaseTelemetryEvent` abstract base class.
pub trait BaseTelemetryEvent {
    /// The PostHog event name, mirroring the Python `event_name` property.
    fn event_name(&self) -> &str;

    /// The PostHog property map, mirroring the Python `properties` property.
    ///
    /// Keys must match `dataclasses.asdict` output one-for-one so dashboards keep
    /// working across the Python and Rust implementations.
    fn properties(&self) -> Map<String, Value>;
}

/// Summary of one agent run, mirroring the Python `AgentTelemetryEvent`.
///
/// Field order follows the Python dataclass so `serde`'s output reads the same
/// way, though JSON object order carries no meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTelemetryEvent {
    pub query: String,
    pub steps: i64,
    pub max_steps: i64,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub use_vision: bool,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
    #[serde(default)]
    pub error_traceback: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_event_name")]
    pub event_name: String,
    #[serde(default)]
    pub is_success: bool,
}

/// `event_name` defaults to `"agent_event"`, matching the dataclass default.
fn default_event_name() -> String {
    AGENT_EVENT_NAME.to_string()
}

impl AgentTelemetryEvent {
    /// Builds a run summary, matching the Python constructor's required fields.
    pub fn new(
        query: impl Into<String>,
        steps: i64,
        max_steps: i64,
        model: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            query: query.into(),
            steps,
            max_steps,
            model: model.into(),
            provider: provider.into(),
            use_vision: false,
            answer: None,
            error: None,
            error_type: None,
            error_traceback: None,
            version: None,
            event_name: default_event_name(),
            is_success: false,
        }
    }

    /// Sets the `use_vision` flag.
    pub fn with_use_vision(mut self, use_vision: bool) -> Self {
        self.use_vision = use_vision;
        self
    }

    /// Sets the final answer.
    pub fn with_answer(mut self, answer: impl Into<String>) -> Self {
        self.answer = Some(answer.into());
        self
    }

    /// Records a failure: message, type name and optional traceback.
    pub fn with_error(
        mut self,
        error: impl Into<String>,
        error_type: impl Into<String>,
        error_traceback: Option<String>,
    ) -> Self {
        self.error = Some(error.into());
        self.error_type = Some(error_type.into());
        self.error_traceback = error_traceback;
        self
    }

    /// Records the crate version; defaults to the crate's [`VERSION`] constant.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Fills `version` with the crate version when the caller did not set one.
    pub fn with_crate_version(self) -> Self {
        if self.version.is_some() {
            return self;
        }
        self.with_version(VERSION)
    }

    /// Sets the `is_success` flag.
    pub fn with_success(mut self, is_success: bool) -> Self {
        self.is_success = is_success;
        self
    }
}

impl BaseTelemetryEvent for AgentTelemetryEvent {
    fn event_name(&self) -> &str {
        if self.event_name.is_empty() {
            AGENT_EVENT_NAME
        } else {
            self.event_name.as_str()
        }
    }

    /// The `asdict`-equivalent payload.
    ///
    /// Every field is present, including `event_name`, so the property names and
    /// null handling match the Python `dataclasses.asdict` output exactly.
    fn properties(&self) -> Map<String, Value> {
        let mut properties = Map::new();
        properties.insert("query".to_string(), Value::String(self.query.clone()));
        properties.insert("steps".to_string(), Value::from(self.steps));
        properties.insert("max_steps".to_string(), Value::from(self.max_steps));
        properties.insert("model".to_string(), Value::String(self.model.clone()));
        properties.insert("provider".to_string(), Value::String(self.provider.clone()));
        properties.insert("use_vision".to_string(), Value::Bool(self.use_vision));
        properties.insert("answer".to_string(), optional_string(&self.answer));
        properties.insert("error".to_string(), optional_string(&self.error));
        properties.insert("error_type".to_string(), optional_string(&self.error_type));
        properties.insert(
            "error_traceback".to_string(),
            optional_string(&self.error_traceback),
        );
        properties.insert("version".to_string(), optional_string(&self.version));
        properties.insert(
            "event_name".to_string(),
            Value::String(self.event_name().to_string()),
        );
        properties.insert("is_success".to_string(), Value::Bool(self.is_success));
        properties
    }
}

/// Converts an optional string to JSON, keeping `null` for `None` like Python.
fn optional_string(value: &Option<String>) -> Value {
    match value {
        Some(text) => Value::String(text.clone()),
        None => Value::Null,
    }
}

/// Object-safe name for [`BaseTelemetryEvent`].
///
/// The Python `capture` accepts any `BaseTelemetryEvent`; Rust expresses the same
/// thing as `&dyn TelemetryEvent`, so this alias keeps call sites readable without
/// introducing a second trait that would make method resolution ambiguous.
pub use BaseTelemetryEvent as TelemetryEvent;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_name_is_agent_event() {
        let event = AgentTelemetryEvent::new("q", 1, 2, "m", "p");
        assert_eq!(event.event_name(), "agent_event");
        assert_eq!(event.event_name, AGENT_EVENT_NAME);
    }

    #[test]
    fn properties_use_the_python_field_names() {
        let event = AgentTelemetryEvent::new("q", 2, 10, "gpt-5.2", "openai")
            .with_use_vision(true)
            .with_answer("done")
            .with_version("0.8.1")
            .with_success(true);

        let properties = event.properties();
        let keys: Vec<&str> = properties.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "query",
                "steps",
                "max_steps",
                "model",
                "provider",
                "use_vision",
                "answer",
                "error",
                "error_type",
                "error_traceback",
                "version",
                "event_name",
                "is_success",
            ]
        );

        assert_eq!(properties["query"], Value::String("q".into()));
        assert_eq!(properties["steps"], Value::from(2));
        assert_eq!(properties["max_steps"], Value::from(10));
        assert_eq!(properties["model"], Value::String("gpt-5.2".into()));
        assert_eq!(properties["provider"], Value::String("openai".into()));
        assert_eq!(properties["use_vision"], Value::Bool(true));
        assert_eq!(properties["answer"], Value::String("done".into()));
        assert_eq!(properties["version"], Value::String("0.8.1".into()));
        assert_eq!(properties["event_name"], Value::String("agent_event".into()));
        assert_eq!(properties["is_success"], Value::Bool(true));
    }

    #[test]
    fn unset_fields_serialize_as_null() {
        let properties = AgentTelemetryEvent::new("q", 0, 0, "m", "p").properties();
        assert_eq!(properties["answer"], Value::Null);
        assert_eq!(properties["error"], Value::Null);
        assert_eq!(properties["error_type"], Value::Null);
        assert_eq!(properties["error_traceback"], Value::Null);
        assert_eq!(properties["version"], Value::Null);
        assert_eq!(properties["use_vision"], Value::Bool(false));
        assert_eq!(properties["is_success"], Value::Bool(false));
    }

    #[test]
    fn error_fields_are_recorded() {
        let event = AgentTelemetryEvent::new("q", 3, 5, "m", "p").with_error(
            "boom",
            "RuntimeError",
            Some("traceback text".to_string()),
        );
        let properties = event.properties();
        assert_eq!(properties["error"], Value::String("boom".into()));
        assert_eq!(properties["error_type"], Value::String("RuntimeError".into()));
        assert_eq!(
            properties["error_traceback"],
            Value::String("traceback text".into())
        );
    }

    #[test]
    fn with_crate_version_fills_the_default_only_when_absent() {
        let event = AgentTelemetryEvent::new("q", 0, 0, "m", "p").with_crate_version();
        assert_eq!(event.version.as_deref(), Some(VERSION));

        let kept = AgentTelemetryEvent::new("q", 0, 0, "m", "p")
            .with_version("1.2.3")
            .with_crate_version();
        assert_eq!(kept.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn properties_match_serialize_output() {
        let event = AgentTelemetryEvent::new("q", 1, 2, "m", "p").with_answer("a");
        let serialized = serde_json::to_value(&event).expect("serialize");
        let properties = Value::Object(event.properties());
        assert_eq!(serialized, properties);
    }

    #[test]
    fn telemetry_event_trait_mirrors_the_base_trait() {
        fn capture_style(event: &dyn TelemetryEvent) -> (String, usize) {
            (event.event_name().to_string(), event.properties().len())
        }

        let event = AgentTelemetryEvent::new("q", 1, 2, "m", "p");
        let (name, count) = capture_style(&event);
        assert_eq!(name, "agent_event");
        assert_eq!(count, 13);
    }

    #[test]
    fn event_name_falls_back_when_blank() {
        let mut event = AgentTelemetryEvent::new("q", 0, 0, "m", "p");
        event.event_name.clear();
        assert_eq!(event.event_name(), AGENT_EVENT_NAME);
        assert_eq!(
            event.properties()["event_name"],
            Value::String(AGENT_EVENT_NAME.to_string())
        );
    }
}
