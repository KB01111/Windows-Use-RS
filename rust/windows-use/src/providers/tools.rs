//! Provider-facing tool schemas and the argument parser shared by every
//! OpenAI-compatible provider.
//!
//! Mirrors `BaseChatLLM.sanitize_schema` from `windows_use/providers/base.py`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A tool as the model sees it: a name, a description and a JSON-schema
/// `parameters` object.
///
/// This replaces the Python `Tool` in the provider layer — the provider only ever
/// needs these three fields (see `Tool.json_schema`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
}

impl ToolSchema {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Builds a schema from the `{name, description, parameters}` object that
    /// `Tool.json_schema` produces in Python.
    pub fn from_json_schema(schema: &Value) -> Self {
        Self {
            name: schema
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            description: schema
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            parameters: schema.get("parameters").cloned().unwrap_or(Value::Null),
        }
    }

    /// The `parameters` object, or an empty object schema when absent.
    pub fn parameters_or_empty(&self) -> Value {
        match &self.parameters {
            Value::Null => serde_json::json!({"type": "object", "properties": {}, "required": []}),
            other => other.clone(),
        }
    }

    /// Serializes back to the `{name, description, parameters}` shape.
    pub fn to_json_schema(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }

    /// The `{"type": "function", "function": {...}}` wrapper used by OpenAI and
    /// every OpenAI-compatible provider.
    pub fn to_openai_tool(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": self.to_json_schema(),
        })
    }

    /// The flat `{"type": "function", ...}` shape used by the Responses APIs.
    pub fn to_responses_tool(&self) -> Value {
        let sanitized = sanitize_tool_schema(self);
        serde_json::json!({
            "type": "function",
            "name": sanitized.name,
            "description": sanitized.description,
            "parameters": sanitized.parameters,
        })
    }
}

/// JSON-schema types the providers accept; anything else is coerced to `string`.
const VALID_TYPES: [&str; 6] = ["string", "integer", "number", "boolean", "array", "object"];

/// Keeps only name/description and reduces every parameter to
/// `type`/`enum`/`description`, exactly like `BaseChatLLM.sanitize_schema`.
pub fn sanitize_schema(tool_schema: &Value) -> Value {
    let params = tool_schema.get("parameters");
    let properties = params
        .and_then(|params| params.get("properties"))
        .and_then(Value::as_object);
    let required = params
        .and_then(|params| params.get("required"))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let mut clean_properties = Map::new();
    if let Some(properties) = properties {
        for (name, property) in properties {
            let (property_type, enum_values, description) = match property {
                Value::Object(_) => {
                    // Perplexity's variant unwraps `anyOf` (Optional types) first.
                    let property_type =
                        resolve_type(property).unwrap_or_else(|| "string".to_string());
                    (
                        property_type,
                        property.get("enum").cloned(),
                        property.get("description").cloned(),
                    )
                }
                _ => ("string".to_string(), None, None),
            };

            let normalized = if VALID_TYPES.contains(&property_type.as_str()) {
                property_type
            } else {
                "string".to_string()
            };

            let mut entry = Map::new();
            entry.insert("type".to_string(), Value::String(normalized));
            if let Some(enum_values) = enum_values {
                entry.insert("enum".to_string(), enum_values);
            }
            if let Some(description) = description {
                entry.insert("description".to_string(), description);
            }
            clean_properties.insert(name.clone(), Value::Object(entry));
        }
    }

    serde_json::json!({
        "name": tool_schema.get("name").cloned().unwrap_or(Value::Null),
        "description": tool_schema.get("description").cloned().unwrap_or(Value::Null),
        "parameters": {
            "type": "object",
            "properties": Value::Object(clean_properties),
            "required": required,
        },
    })
}

/// Resolves a property's JSON-schema type, unwrapping `anyOf` for optional
/// fields (the Perplexity provider does this too).
fn resolve_type(property: &Value) -> Option<String> {
    if let Some(any_of) = property.get("anyOf").and_then(Value::as_array) {
        let non_null: Vec<&Value> = any_of
            .iter()
            .filter(|option| option.get("type").and_then(Value::as_str) != Some("null"))
            .collect();
        return non_null
            .first()
            .and_then(|option| option.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some("string".to_string()));
    }
    property
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some("string".to_string()))
}

/// Typed form of [`sanitize_schema`].
pub fn sanitize_tool_schema(tool_schema: &ToolSchema) -> ToolSchema {
    let sanitized = sanitize_schema(&tool_schema.to_json_schema());
    ToolSchema::from_json_schema(&sanitized)
}

/// Parses tool-call arguments into a parameter map.
///
/// Accepts a JSON object string (the OpenAI/Anthropic wire format), an already
/// decoded object, or nothing. Malformed JSON yields an empty map, matching the
/// Python `except json.JSONDecodeError: params = {}` behaviour, and never panics.
pub fn parse_tool_arguments(arguments: Option<&Value>) -> Map<String, Value> {
    match arguments {
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Map::new();
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            }
        }
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    }
}

/// Convenience wrapper for providers that hold the raw argument string.
pub fn parse_tool_arguments_str(arguments: &str) -> Map<String, Value> {
    parse_tool_arguments(Some(&Value::String(arguments.to_string())))
}

/// Serializes a parameter map back to a JSON string for replay in history,
/// mirroring Python's `json.dumps(msg.params)`.
pub fn serialize_tool_arguments(params: &Map<String, Value>) -> String {
    serde_json::to_string(&Value::Object(params.clone())).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        serde_json::json!({
            "name": "launch_app",
            "description": "Launch an application",
            "parameters": {
                "type": "object",
                "title": "LaunchAppArgs",
                "properties": {
                    "app_name": {"type": "string", "description": "The app", "title": "App"},
                    "count": {"type": "integer"},
                    "mode": {"type": "string", "enum": ["fast", "safe"]},
                    "flag": {"description": "no type"},
                    "odd": {"type": "null"},
                    "blob": {"type": "object", "description": "nested"},
                },
                "required": ["app_name"],
            },
        })
    }

    #[test]
    fn sanitize_keeps_only_type_enum_description() {
        let result = sanitize_schema(&schema());
        assert_eq!(result["name"], "launch_app");
        assert_eq!(result["description"], "Launch an application");
        assert_eq!(result["parameters"]["type"], "object");
        assert_eq!(result["parameters"]["required"], serde_json::json!(["app_name"]));

        let properties = result["parameters"]["properties"].as_object().unwrap();
        assert_eq!(properties["app_name"]["type"], "string");
        assert_eq!(properties["app_name"]["description"], "The app");
        // `title` is dropped.
        assert!(properties["app_name"].get("title").is_none());
        assert_eq!(properties["count"]["type"], "integer");
        assert_eq!(properties["mode"]["enum"], serde_json::json!(["fast", "safe"]));
        // Missing type defaults to string.
        assert_eq!(properties["flag"]["type"], "string");
        // Unknown/unsupported types normalize to string.
        assert_eq!(properties["odd"]["type"], "string");
        assert_eq!(properties["blob"]["type"], "object");
        assert_eq!(properties["blob"]["description"], "nested");
    }

    #[test]
    fn sanitize_handles_missing_parameters() {
        let result = sanitize_schema(&serde_json::json!({"name": "noop"}));
        assert_eq!(result["name"], "noop");
        assert!(result["description"].is_null());
        assert_eq!(result["parameters"]["type"], "object");
        assert_eq!(result["parameters"]["properties"], serde_json::json!({}));
        assert_eq!(result["parameters"]["required"], serde_json::json!([]));
    }

    #[test]
    fn sanitize_unwraps_any_of() {
        let result = sanitize_schema(&serde_json::json!({
            "name": "maybe",
            "description": "d",
            "parameters": {
                "type": "object",
                "properties": {
                    "value": {
                        "anyOf": [{"type": "null"}, {"type": "integer"}],
                        "description": "opt"
                    }
                },
                "required": []
            }
        }));
        assert_eq!(result["parameters"]["properties"]["value"]["type"], "integer");
    }

    #[test]
    fn sanitize_tool_schema_round_trips() {
        let tool = ToolSchema::from_json_schema(&schema());
        let sanitized = sanitize_tool_schema(&tool);
        assert_eq!(sanitized.name, "launch_app");
        assert_eq!(sanitized.parameters["properties"]["count"]["type"], "integer");
    }

    #[test]
    fn tool_call_args_parse_from_string() {
        let params = parse_tool_arguments(Some(&serde_json::json!("{\"x\": 1, \"label\": \"ok\"}")));
        assert_eq!(params.get("x"), Some(&serde_json::json!(1)));
        assert_eq!(params.get("label"), Some(&serde_json::json!("ok")));
    }

    #[test]
    fn invalid_tool_call_args_yield_empty_map() {
        assert!(parse_tool_arguments(Some(&serde_json::json!("{not json"))).is_empty());
        assert!(parse_tool_arguments(Some(&serde_json::json!("[1,2,3]"))).is_empty());
        assert!(parse_tool_arguments(Some(&serde_json::json!(""))).is_empty());
        assert!(parse_tool_arguments(None).is_empty());
        assert!(parse_tool_arguments(Some(&serde_json::json!(123))).is_empty());
    }

    #[test]
    fn tool_call_args_accept_decoded_objects() {
        let params = parse_tool_arguments(Some(&serde_json::json!({"a": true})));
        assert_eq!(params.get("a"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn arguments_round_trip() {
        let params = parse_tool_arguments_str("{\"x\":1}");
        let serialized = serialize_tool_arguments(&params);
        assert_eq!(serialized, "{\"x\":1}");
    }

    #[test]
    fn openai_tool_wrapper_shape() {
        let tool = ToolSchema::new("click", "Click", serde_json::json!({"type": "object"}));
        let wrapped = tool.to_openai_tool();
        assert_eq!(wrapped["type"], "function");
        assert_eq!(wrapped["function"]["name"], "click");
        assert_eq!(wrapped["function"]["parameters"]["type"], "object");
    }
}
