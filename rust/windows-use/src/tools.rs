//! Tool parameter schemas and validation, mirroring `windows_use/tools/service.py`.
//!
//! The provider-facing [`ToolSchema`] type lives in [`crate::providers::tools`]; it is
//! re-exported here so tool building and the agent share one type.

use serde_json::{json, Map, Value};

pub use crate::providers::tools::ToolSchema;

/// Properties dropped from the generated JSON schema.
const EXCLUDED_PROPERTIES: [&str; 1] = ["title"];

/// Maximum tool output length.
pub const MAX_TOOL_OUTPUT_LENGTH: usize = 10_000;

/// A tool result, matching the registry's `ToolResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub is_success: bool,
    pub content: Option<String>,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self { is_success: true, content: Some(content.into()), error: None }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self { is_success: false, content: None, error: Some(error.into()) }
    }
}

/// Removes `title` keys recursively, as the Python `exclude_properties` helper does.
fn exclude_properties(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut result = Map::new();
            for (key, entry) in map {
                if EXCLUDED_PROPERTIES.contains(&key.as_str()) {
                    continue;
                }
                result.insert(key, exclude_properties(entry));
            }
            Value::Object(result)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(exclude_properties).collect()),
        other => other,
    }
}

/// Parameter description for a tool field.
#[derive(Debug, Clone)]
pub struct ParamSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    pub required: bool,
    /// Example values, mirroring pydantic's `Field(examples=[...])`.
    pub examples: Option<Value>,
}

impl ParamSpec {
    fn with_schema(
        name: &'static str,
        description: &'static str,
        schema: Value,
        required: bool,
    ) -> Self {
        Self { name, description, schema, required, examples: None }
    }

    /// Attaches the pydantic-style `examples` list.
    pub fn examples(mut self, examples: Value) -> Self {
        self.examples = Some(examples);
        self
    }

    /// A required string parameter.
    pub fn string(name: &'static str, description: &'static str) -> Self {
        Self::with_schema(name, description, json!({ "type": "string" }), true)
    }

    /// An optional string parameter.
    pub fn optional_string(name: &'static str, description: &'static str) -> Self {
        Self::with_schema(name, description, json!({ "type": "string" }), false)
    }

    /// An integer parameter with a default.
    pub fn integer(name: &'static str, description: &'static str, default: i64, required: bool) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "type": "integer", "default": default }),
            required,
        )
    }

    /// A boolean parameter with a default.
    pub fn boolean(name: &'static str, description: &'static str, default: bool) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "type": "boolean", "default": default }),
            false,
        )
    }

    /// A string parameter restricted to an enum.
    pub fn enumerated(
        name: &'static str,
        description: &'static str,
        values: &[&str],
        default: Option<&str>,
        required: bool,
    ) -> Self {
        let mut schema = json!({ "type": "string", "enum": values });
        if let Some(default) = default {
            schema["default"] = json!(default);
        }
        Self::with_schema(name, description, schema, required)
    }

    /// An integer parameter restricted to an enum (e.g. `clicks`).
    pub fn enumerated_int(
        name: &'static str,
        description: &'static str,
        values: &[i64],
        default: i64,
        required: bool,
    ) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "type": "integer", "enum": values, "default": default }),
            required,
        )
    }

    /// An array of integers (coordinate pairs).
    pub fn int_array(name: &'static str, description: &'static str, required: bool) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "type": "array", "items": { "type": "integer" } }),
            required,
        )
    }

    /// A nested array parameter (e.g. lists of coordinate pairs).
    pub fn array_of_arrays(
        name: &'static str,
        description: &'static str,
        inner: Value,
    ) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "type": "array", "items": { "type": "array", "items": inner } }),
            true,
        )
    }

    /// A nullable parameter described with `anyOf`, matching pydantic's output.
    pub fn nullable(name: &'static str, description: &'static str, inner: Value) -> Self {
        Self::with_schema(
            name,
            description,
            json!({ "anyOf": [inner, { "type": "null" }], "default": null }),
            false,
        )
    }
}

/// Builds a tool's `parameters` object from its specs.
pub fn build_parameters(params: &[ParamSpec]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for param in params {
        let mut schema = param.schema.clone();
        if let Value::Object(map) = &mut schema {
            map.insert("description".to_string(), json!(param.description));
            if let Some(examples) = &param.examples {
                map.insert("examples".to_string(), examples.clone());
            }
        }
        properties.insert(param.name.to_string(), schema);
        if param.required {
            required.push(Value::String(param.name.to_string()));
        }
    }

    exclude_properties(json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
}

/// Validates tool parameters, mirroring pydantic's required-field and type checks.
///
/// Returns `field:message` strings exactly like `Tool.validate_params`.
pub fn validate_params(params: &[ParamSpec], args: &Map<String, Value>) -> Vec<String> {
    let mut errors = Vec::new();

    for param in params {
        let value = args.get(param.name);
        if param.required {
            match value {
                None | Some(Value::Null) => {
                    errors.push(format!("{}:Field required", param.name));
                    continue;
                }
                _ => {}
            }
        }
        let Some(value) = value else { continue };
        if value.is_null() {
            continue;
        }

        let type_ok = match value {
            Value::String(_) => param.schema.get("type").and_then(Value::as_str) == Some("string")
                || param.schema.get("anyOf").is_some()
                || param
                    .schema
                    .get("anyOf")
                    .and_then(Value::as_array)
                    .map(|variants| variants.iter().any(|variant| variant.get("type").and_then(Value::as_str) == Some("string")))
                    .unwrap_or(false),
            Value::Bool(_) => param.schema.get("type").and_then(Value::as_str) == Some("boolean"),
            Value::Number(number) => {
                if number.is_i64() {
                    param.schema.get("type").and_then(Value::as_str) == Some("integer")
                        || param.schema.get("type").and_then(Value::as_str) == Some("number")
                } else {
                    param.schema.get("type").and_then(Value::as_str) == Some("number")
                }
            }
            Value::Array(items) => {
                let is_array = param.schema.get("type").and_then(Value::as_str) == Some("array");
                if is_array {
                    if let Some(item_type) = param
                        .schema
                        .get("items")
                        .and_then(|items| items.get("type"))
                        .and_then(Value::as_str)
                    {
                        items.iter().all(|item| match item_type {
                            "integer" => item.is_i64(),
                            "string" => item.is_string(),
                            _ => true,
                        })
                    } else {
                        true
                    }
                } else {
                    false
                }
            }
            Value::Object(_) => param.schema.get("type").and_then(Value::as_str) == Some("object"),
            Value::Null => true,
        };

        if !type_ok {
            errors.push(format!(
                "{}:Input should be a valid {}",
                param.name,
                type_name(&param.schema)
            ));
            continue;
        }

        // Enum membership.
        if let Some(allowed) = param.schema.get("enum").and_then(Value::as_array) {
            if let Some(text) = value.as_str() {
                if !allowed.iter().any(|item| item.as_str() == Some(text)) {
                    errors.push(format!("{}:Input should be a valid enum value", param.name));
                }
            }
        }
    }

    errors
}

fn type_name(schema: &Value) -> &'static str {
    match schema.get("type").and_then(Value::as_str) {
        Some("integer") => "integer",
        Some("number") => "number",
        Some("boolean") => "boolean",
        Some("array") => "list",
        Some("object") => "dict",
        _ => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_specs() -> Vec<ParamSpec> {
        vec![
            ParamSpec::string("thought", "reasoning"),
            ParamSpec::int_array("loc", "coordinates", true),
            ParamSpec::enumerated("button", "mouse button", &["left", "right", "middle"], Some("left"), false),
            ParamSpec::integer("clicks", "click count", 1, false),
        ]
    }

    #[test]
    fn parameters_exclude_titles() {
        let parameters = build_parameters(&click_specs());
        let rendered = parameters.to_string();
        assert!(!rendered.contains("\"title\""));
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["required"], json!(["thought", "loc"]));
        assert_eq!(parameters["properties"]["button"]["default"], "left");
    }

    #[test]
    fn missing_required_field_reports_pydantic_style_error() {
        let specs = click_specs();
        let mut args = Map::new();
        args.insert("thought".to_string(), json!("ok"));
        let errors = validate_params(&specs, &args);
        assert_eq!(errors, vec!["loc:Field required".to_string()]);
    }

    #[test]
    fn wrong_types_are_rejected() {
        let specs = click_specs();
        let mut args = Map::new();
        args.insert("thought".to_string(), json!("ok"));
        args.insert("loc".to_string(), json!("not-a-list"));
        let errors = validate_params(&specs, &args);
        assert_eq!(errors, vec!["loc:Input should be a valid list".to_string()]);
    }

    #[test]
    fn invalid_enum_value_is_rejected() {
        let specs = click_specs();
        let mut args = Map::new();
        args.insert("thought".to_string(), json!("ok"));
        args.insert("loc".to_string(), json!([1, 2]));
        args.insert("button".to_string(), json!("sideways"));
        let errors = validate_params(&specs, &args);
        assert_eq!(errors, vec!["button:Input should be a valid enum value".to_string()]);
    }

    #[test]
    fn integer_arrays_require_integer_items() {
        let specs = click_specs();
        let mut args = Map::new();
        args.insert("thought".to_string(), json!("ok"));
        args.insert("loc".to_string(), json!([1.5, 2]));
        let errors = validate_params(&specs, &args);
        assert_eq!(errors, vec!["loc:Input should be a valid list".to_string()]);
    }

    #[test]
    fn valid_arguments_pass() {
        let specs = click_specs();
        let mut args = Map::new();
        args.insert("thought".to_string(), json!("clicking"));
        args.insert("loc".to_string(), json!([640, 360]));
        args.insert("button".to_string(), json!("right"));
        args.insert("clicks".to_string(), json!(2));
        assert!(validate_params(&specs, &args).is_empty());
    }
}
