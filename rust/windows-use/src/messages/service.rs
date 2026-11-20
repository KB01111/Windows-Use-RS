//! Message models, mirroring `windows_use/messages/service.py`.
//!
//! The Python port keeps a pydantic class hierarchy (`SystemMessage`,
//! `HumanMessage`, `ImageMessage`, `AIMessage`, `ToolMessage`). Here they are
//! variants of a single [`BaseMessage`] enum so a message list can be a
//! `Vec<BaseMessage>` and every provider can match on the role.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use image::{ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Hard ceiling for a single encoded image (`ImageMessage._MAX_IMAGE_BYTES`).
pub const MAX_IMAGE_BYTES: usize = 4_800_000;
/// Smallest width the downscale ladder will produce.
pub const MIN_IMAGE_WIDTH: u32 = 320;
/// Smallest height the downscale ladder will produce.
pub const MIN_IMAGE_HEIGHT: u32 = 240;
/// Default image mime type used by `ImageMessage`.
pub const DEFAULT_MIME_TYPE: &str = "image/png";

/// Conversation role, serialized exactly like the Python `Literal` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Human,
    Ai,
    Tool,
}

impl Role {
    /// The wire value used by the Python port (`"system" | "human" | "ai" | "tool"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::Human => "human",
            Role::Ai => "ai",
            Role::Tool => "tool",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Token usage reported by a provider, matching `messages.Usage`.
///
/// `providers::views::TokenUsage` is the provider-facing twin: same fields but
/// plain `i64` values that default to `0`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i64>,
}

impl Usage {
    pub fn new(prompt_tokens: i64, completion_tokens: i64, total_tokens: i64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            ..Self::default()
        }
    }
}

/// A conversation message.
///
/// `content` of an `ImageMessage` is the accompanying text; the raw encoded
/// image bytes live in `images` and are compressed on demand with
/// [`BaseMessage::convert_images_base64`].
#[derive(Debug, Clone)]
pub enum BaseMessage {
    SystemMessage {
        content: String,
    },
    HumanMessage {
        content: String,
    },
    /// A human turn carrying images; serialized as `role: "human"` like Python.
    ImageMessage {
        content: String,
        images: Vec<Vec<u8>>,
        mime_type: String,
    },
    AIMessage {
        content: Option<String>,
        thinking: Option<String>,
        thinking_signature: Option<String>,
        usage: Option<Usage>,
    },
    ToolMessage {
        id: String,
        name: String,
        params: Map<String, Value>,
        content: Option<String>,
        thinking: Option<String>,
        thinking_signature: Option<String>,
        usage: Option<Usage>,
    },
}

impl Serialize for BaseMessage {
    /// Serializes with the same shape as `model_dump(mode="json")`, except that
    /// image bytes are never emitted (they are not JSON-serializable in Python
    /// either — see `ImageMessage.to_dict`).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        match self {
            BaseMessage::SystemMessage { content } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("role", Role::System.as_str())?;
                map.serialize_entry("content", content)?;
                map.end()
            }
            BaseMessage::HumanMessage { content } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("role", Role::Human.as_str())?;
                map.serialize_entry("content", content)?;
                map.end()
            }
            BaseMessage::ImageMessage {
                content, mime_type, ..
            } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("role", Role::Human.as_str())?;
                map.serialize_entry("content", content)?;
                map.serialize_entry("mime_type", mime_type)?;
                map.end()
            }
            BaseMessage::AIMessage {
                content,
                thinking,
                thinking_signature,
                usage,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("role", Role::Ai.as_str())?;
                if let Some(content) = content {
                    map.serialize_entry("content", content)?;
                }
                if let Some(thinking) = thinking {
                    map.serialize_entry("thinking", thinking)?;
                }
                if let Some(signature) = thinking_signature {
                    map.serialize_entry("thinking_signature", signature)?;
                }
                if let Some(usage) = usage {
                    map.serialize_entry("usage", usage)?;
                }
                map.end()
            }
            BaseMessage::ToolMessage {
                id,
                name,
                params,
                content,
                thinking,
                thinking_signature,
                usage,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("role", Role::Tool.as_str())?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("params", params)?;
                if let Some(content) = content {
                    map.serialize_entry("content", content)?;
                }
                if let Some(thinking) = thinking {
                    map.serialize_entry("thinking", thinking)?;
                }
                if let Some(signature) = thinking_signature {
                    map.serialize_entry("thinking_signature", signature)?;
                }
                if let Some(usage) = usage {
                    map.serialize_entry("usage", usage)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for BaseMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(BaseMessage::from_dict(&value))
    }
}

/// Result of compressing one image: the encoded payload plus its mime type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedImage {
    pub data: Vec<u8>,
    pub mime_type: String,
}

impl CompressedImage {
    pub fn to_base64(&self) -> String {
        BASE64_STANDARD.encode(&self.data)
    }
}

impl BaseMessage {
    /// The role of the message. `ImageMessage` reports `Human`, like Python.
    pub fn role(&self) -> Role {
        match self {
            BaseMessage::SystemMessage { .. } => Role::System,
            BaseMessage::HumanMessage { .. } | BaseMessage::ImageMessage { .. } => Role::Human,
            BaseMessage::AIMessage { .. } => Role::Ai,
            BaseMessage::ToolMessage { .. } => Role::Tool,
        }
    }

    /// The text content, when the variant carries one.
    pub fn content(&self) -> Option<&str> {
        match self {
            BaseMessage::SystemMessage { content }
            | BaseMessage::HumanMessage { content }
            | BaseMessage::ImageMessage { content, .. } => Some(content.as_str()),
            BaseMessage::AIMessage { content, .. } | BaseMessage::ToolMessage { content, .. } => {
                content.as_deref()
            }
        }
    }

    /// Reasoning content attached to an AI/tool message, when present.
    pub fn thinking(&self) -> Option<&str> {
        match self {
            BaseMessage::AIMessage { thinking, .. } | BaseMessage::ToolMessage { thinking, .. } => {
                thinking.as_deref()
            }
            _ => None,
        }
    }

    /// The cryptographic thinking signature (Anthropic / Gemini), when present.
    pub fn thinking_signature(&self) -> Option<&str> {
        match self {
            BaseMessage::AIMessage {
                thinking_signature, ..
            }
            | BaseMessage::ToolMessage {
                thinking_signature, ..
            } => thinking_signature.as_deref(),
            _ => None,
        }
    }

    /// Token usage attached to the message, when present.
    pub fn usage(&self) -> Option<&Usage> {
        match self {
            BaseMessage::AIMessage { usage, .. } | BaseMessage::ToolMessage { usage, .. } => {
                usage.as_ref()
            }
            _ => None,
        }
    }

    /// Raw images carried by the message (empty for every other variant).
    pub fn images(&self) -> &[Vec<u8>] {
        match self {
            BaseMessage::ImageMessage { images, .. } => images,
            _ => &[],
        }
    }

    /// Mime type declared for the message's images.
    pub fn mime_type(&self) -> &str {
        match self {
            BaseMessage::ImageMessage { mime_type, .. } => mime_type,
            _ => DEFAULT_MIME_TYPE,
        }
    }

    /// Builds a plain human turn.
    pub fn human(content: impl Into<String>) -> Self {
        BaseMessage::HumanMessage {
            content: content.into(),
        }
    }

    /// Builds a system prompt.
    pub fn system(content: impl Into<String>) -> Self {
        BaseMessage::SystemMessage {
            content: content.into(),
        }
    }

    /// Builds an assistant turn.
    pub fn ai(content: impl Into<String>) -> Self {
        BaseMessage::AIMessage {
            content: Some(content.into()),
            thinking: None,
            thinking_signature: None,
            usage: None,
        }
    }

    /// Builds a tool result turn.
    pub fn tool_result(id: &str, name: &str, params: Map<String, Value>, content: &str) -> Self {
        BaseMessage::ToolMessage {
            id: id.to_string(),
            name: name.to_string(),
            params,
            content: Some(content.to_string()),
            thinking: None,
            thinking_signature: None,
            usage: None,
        }
    }

    /// Serializes to a JSON object, dropping image payloads exactly like
    /// `ImageMessage.to_dict` does in Python.
    pub fn to_dict(&self) -> Value {
        match self {
            BaseMessage::ImageMessage { content, .. } => serde_json::json!({
                "role": Role::Human.as_str(),
                "content": content,
            }),
            other => serde_json::to_value(other).unwrap_or(Value::Null),
        }
    }

    /// Deserializes a message, dispatching on `role` like `BaseMessage.from_dict`.
    /// Unknown roles fall back to a human message.
    ///
    /// Parsing is done field by field (rather than through `serde_json::from_value`)
    /// so this stays the single reference implementation for both the `Deserialize`
    /// impl above and direct callers.
    pub fn from_dict(value: &Value) -> Self {
        match value.get("role").and_then(Value::as_str) {
            Some("system") => BaseMessage::SystemMessage {
                content: string_field(value, "content"),
            },
            Some("ai") => BaseMessage::AIMessage {
                content: optional_string_field(value, "content"),
                thinking: optional_string_field(value, "thinking"),
                thinking_signature: optional_string_field(value, "thinking_signature"),
                usage: parse_usage_value(value.get("usage")),
            },
            Some("tool") => BaseMessage::ToolMessage {
                id: string_field(value, "id"),
                name: string_field(value, "name"),
                params: value
                    .get("params")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
                content: optional_string_field(value, "content"),
                thinking: optional_string_field(value, "thinking"),
                thinking_signature: optional_string_field(value, "thinking_signature"),
                usage: parse_usage_value(value.get("usage")),
            },
            Some("human") => BaseMessage::HumanMessage {
                content: string_field(value, "content"),
            },
            _ => BaseMessage::HumanMessage {
                content: string_field(value, "content"),
            },
        }
    }

    /// Compresses every image and returns the encoded payloads together with the
    /// mime type the payloads actually use.
    ///
    /// Mirrors `ImageMessage.convert_images(format="bytes")`: the ladder is
    /// "keep the original encoding" → JPEG q85 → JPEG 80/60/40/25 → five
    /// halvings at q50 with a 320x240 floor.
    pub fn convert_images_bytes_with_mime(&self) -> (Vec<Vec<u8>>, String) {
        let mut mime_type = self.mime_type().to_string();
        let mut payloads = Vec::with_capacity(self.images().len());
        for image_bytes in self.images() {
            let compressed = compress_image(image_bytes, &mime_type, MAX_IMAGE_BYTES);
            mime_type = compressed.mime_type;
            payloads.push(compressed.data);
        }
        (payloads, mime_type)
    }

    /// Same as [`BaseMessage::convert_images_bytes_with_mime`] but base64 encoded.
    pub fn convert_images_base64_with_mime(&self) -> (Vec<String>, String) {
        let (payloads, mime_type) = self.convert_images_bytes_with_mime();
        (
            payloads.iter().map(|bytes| BASE64_STANDARD.encode(bytes)).collect(),
            mime_type,
        )
    }

    /// Mirrors `ImageMessage.convert_images(format="bytes")`.
    pub fn convert_images_bytes(&self) -> Vec<Vec<u8>> {
        self.convert_images_bytes_with_mime().0
    }

    /// Mirrors `ImageMessage.convert_images(format="base64")`.
    pub fn convert_images_base64(&self) -> Vec<String> {
        self.convert_images_base64_with_mime().0
    }

    /// The mime type the images will end up with after compression.
    pub fn effective_mime_type(&self) -> String {
        self.convert_images_bytes_with_mime().1
    }

    /// Scales every carried image by `scale` (Pillow's `Image.resize`), keeping
    /// the original encoding. Undecodable payloads are left untouched.
    pub fn scale_images(&mut self, scale: f64) {
        if let BaseMessage::ImageMessage {
            images, mime_type, ..
        } = self
        {
            for image_bytes in images.iter_mut() {
                let Ok(decoded) = image::load_from_memory(image_bytes) else {
                    continue;
                };
                let width = (decoded.width() as f64 * scale).max(1.0) as u32;
                let height = (decoded.height() as f64 * scale).max(1.0) as u32;
                let resized = decoded.resize_exact(width, height, image::imageops::FilterType::CatmullRom);
                if let Some(encoded) = encode_like(&resized, mime_type) {
                    *image_bytes = encoded;
                }
            }
        }
    }
}

/// Re-encodes a decoded image using the container implied by `mime_type`.
fn encode_like(decoded: &image::DynamicImage, mime_type: &str) -> Option<Vec<u8>> {
    match normalized_format(mime_type).as_str() {
        "PNG" => encode_png(decoded),
        _ => encode_jpeg(decoded, 85),
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Reads a string field, distinguishing "absent" from "present but empty".
fn optional_string_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

/// Parses a `usage` object into [`Usage`], tolerating missing fields.
fn parse_usage_value(value: Option<&Value>) -> Option<Usage> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    let number = |key: &str| -> Option<i64> {
        match value.get(key) {
            Some(Value::Number(number)) => number.as_i64(),
            Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
            _ => None,
        }
    };
    Some(Usage {
        prompt_tokens: number("prompt_tokens").unwrap_or(0),
        completion_tokens: number("completion_tokens").unwrap_or(0),
        total_tokens: number("total_tokens").unwrap_or(0),
        image_tokens: number("image_tokens"),
        thinking_tokens: number("thinking_tokens"),
        cache_creation_input_tokens: number("cache_creation_input_tokens"),
        cache_read_input_tokens: number("cache_read_input_tokens"),
    })
}

/// `mime_type.split("/")[-1].upper()`, with `JPG` folded into `JPEG`.
fn normalized_format(mime_type: &str) -> String {
    let tail = mime_type.rsplit('/').next().unwrap_or(mime_type);
    let upper = tail.to_uppercase();
    if upper == "JPG" {
        "JPEG".to_string()
    } else {
        upper
    }
}

fn encode_jpeg(decoded: &image::DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let rgb = decoded.to_rgb8();
    let mut buffer = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, quality);
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(buffer)
}

fn encode_png(decoded: &image::DynamicImage) -> Option<Vec<u8>> {
    let rgba = decoded.to_rgba8();
    let mut buffer = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buffer)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(buffer)
}

/// Port of `ImageMessage._compress_image`.
///
/// Returns the (possibly re-encoded) payload and the mime type it is in. Payloads
/// that cannot be decoded are passed through untouched so a provider call never
/// panics on odd input.
pub fn compress_image(data: &[u8], mime_type: &str, max_bytes: usize) -> CompressedImage {
    let original_mime = mime_type.to_string();
    let declared = normalized_format(mime_type);

    // PNG passthrough: Pillow re-saves PNG without quality steps and the bytes
    // stay identical, so only fall back to JPEG when the payload is too large.
    if declared == "PNG" && data.len() <= max_bytes {
        return CompressedImage {
            data: data.to_vec(),
            mime_type: original_mime,
        };
    }

    let Ok(decoded) = image::load_from_memory(data) else {
        return CompressedImage {
            data: data.to_vec(),
            mime_type: original_mime,
        };
    };

    // First attempt: keep the declared format at quality 85.
    let first = match declared.as_str() {
        "PNG" => encode_png(&decoded),
        _ => encode_jpeg(&decoded, 85),
    };
    if let Some(payload) = first {
        if payload.len() <= max_bytes {
            return CompressedImage {
                data: payload,
                mime_type: original_mime,
            };
        }
    }

    // Fall back to JPEG quality steps.
    let actual_mime = "image/jpeg".to_string();
    let mut last: Option<Vec<u8>> = None;
    for quality in [80, 60, 40, 25] {
        if let Some(payload) = encode_jpeg(&decoded, quality) {
            if payload.len() <= max_bytes {
                return CompressedImage {
                    data: payload,
                    mime_type: actual_mime,
                };
            }
            last = Some(payload);
        }
    }

    // Shrink the dimensions up to five times, floor 320x240, at quality 50.
    let mut current = decoded;
    for _ in 0..5 {
        let new_width = (current.width() / 2).max(MIN_IMAGE_WIDTH);
        let new_height = (current.height() / 2).max(MIN_IMAGE_HEIGHT);
        current = current.resize_exact(new_width, new_height, image::imageops::FilterType::CatmullRom);
        if let Some(payload) = encode_jpeg(&current, 50) {
            if payload.len() <= max_bytes {
                return CompressedImage {
                    data: payload,
                    mime_type: actual_mime,
                };
            }
            last = Some(payload);
        }
    }

    CompressedImage {
        data: last.unwrap_or_else(|| data.to_vec()),
        mime_type: actual_mime,
    }
}

/// `textwrap.shorten` equivalent used by the Python `__repr__` methods.
fn shorten(text: &str, width: usize, placeholder: &str) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(placeholder.chars().count());
    let mut trimmed = String::new();
    for word in text.split_whitespace() {
        let candidate_len = if trimmed.is_empty() {
            word.chars().count()
        } else {
            trimmed.chars().count() + 1 + word.chars().count()
        };
        if candidate_len > keep {
            break;
        }
        if !trimmed.is_empty() {
            trimmed.push(' ');
        }
        trimmed.push_str(word);
    }
    format!("{trimmed}{placeholder}")
}

impl std::fmt::Display for BaseMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaseMessage::SystemMessage { content } => {
                write!(formatter, "SystemMessage(content={})", shorten(content, 80, "..."))
            }
            BaseMessage::HumanMessage { content } => {
                write!(formatter, "HumanMessage(content={})", shorten(content, 80, "..."))
            }
            BaseMessage::ImageMessage {
                content,
                images,
                mime_type,
            } => write!(
                formatter,
                "ImageMessage(content={}, images={}, mime_type={})",
                shorten(content, 80, "..."),
                images.len(),
                mime_type
            ),
            BaseMessage::AIMessage {
                content, thinking, ..
            } => write!(
                formatter,
                "AIMessage(content={}, thinking={})",
                content.as_deref().unwrap_or("None"),
                shorten(thinking.as_deref().unwrap_or("None"), 50, "...")
            ),
            BaseMessage::ToolMessage {
                id,
                name,
                params,
                content,
                thinking,
                ..
            } => write!(
                formatter,
                "ToolMessage(name={}, id={}, params={}, content={}, thinking={})",
                name,
                id,
                Value::Object(params.clone()),
                shorten(content.as_deref().unwrap_or(""), 80, "..."),
                shorten(thinking.as_deref().unwrap_or("None"), 50, "...")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_png(width: u32, height: u32) -> Vec<u8> {
        let mut buffer = Vec::new();
        let pixels = vec![128u8; (width * height * 3) as usize];
        image::codecs::png::PngEncoder::new(&mut buffer)
            .write_image(
                &pixels,
                width,
                height,
                ExtendedColorType::Rgb8,
            )
            .expect("png encodes");
        buffer
    }

    #[test]
    fn role_wire_values_match_python() {
        assert_eq!(Role::System.as_str(), "system");
        assert_eq!(Role::Human.as_str(), "human");
        assert_eq!(Role::Ai.as_str(), "ai");
        assert_eq!(Role::Tool.as_str(), "tool");
        assert_eq!(serde_json::to_string(&Role::Ai).unwrap(), "\"ai\"");
        assert_eq!(serde_json::to_string(&Role::Human).unwrap(), "\"human\"");
    }

    #[test]
    fn image_message_serializes_as_human_without_payload() {
        let message = BaseMessage::ImageMessage {
            content: "screenshot".to_string(),
            images: vec![vec![1, 2, 3]],
            mime_type: DEFAULT_MIME_TYPE.to_string(),
        };
        assert_eq!(message.role(), Role::Human);
        assert_eq!(
            message.to_dict(),
            serde_json::json!({"role": "human", "content": "screenshot"})
        );
    }

    #[test]
    fn small_png_is_passed_through() {
        let png = encoded_png(8, 8);
        let compressed = compress_image(&png, DEFAULT_MIME_TYPE, MAX_IMAGE_BYTES);
        assert_eq!(compressed.mime_type, "image/png");
        assert_eq!(compressed.data, png);
    }

    #[test]
    fn oversized_jpeg_is_recompressed_under_the_limit() {
        let mut buffer = Vec::new();
        let pixels: Vec<u8> = (0..512 * 512 * 3).map(|index| (index % 251) as u8).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, 100)
            .write_image(&pixels, 512, 512, ExtendedColorType::Rgb8)
            .expect("jpeg encodes");
        let compressed = compress_image(&buffer, "image/jpeg", 20_000);
        assert!(compressed.data.len() <= 20_000, "got {} bytes", compressed.data.len());
        assert_eq!(compressed.mime_type, "image/jpeg");
    }

    #[test]
    fn undecodable_payload_survives_compression() {
        let compressed = compress_image(b"not an image", "image/jpeg", 10);
        assert_eq!(compressed.data, b"not an image".to_vec());
    }

    #[test]
    fn image_message_exposes_base64_and_mime() {
        let message = BaseMessage::ImageMessage {
            content: String::new(),
            images: vec![encoded_png(4, 4)],
            mime_type: DEFAULT_MIME_TYPE.to_string(),
        };
        let (encoded, mime_type) = message.convert_images_base64_with_mime();
        assert_eq!(encoded.len(), 1);
        assert_eq!(mime_type, "image/png");
        assert!(!message.convert_images_base64().is_empty());
    }

    #[test]
    fn from_dict_dispatches_on_role() {
        let system = BaseMessage::from_dict(&serde_json::json!({
            "role": "system",
            "content": "be brief"
        }));
        assert!(matches!(system, BaseMessage::SystemMessage { .. }));

        let unknown = BaseMessage::from_dict(&serde_json::json!({"role": "wizard", "content": "x"}));
        assert!(matches!(unknown, BaseMessage::HumanMessage { .. }));

        let tool = BaseMessage::from_dict(&serde_json::json!({
            "role": "tool",
            "id": "call_1",
            "name": "click",
            "params": {"x": 1},
            "content": "ok"
        }));
        assert!(matches!(tool, BaseMessage::ToolMessage { .. }));
    }

    #[test]
    fn round_trip_through_serde() {
        let message = BaseMessage::AIMessage {
            content: Some("hello".to_string()),
            thinking: Some("considering".to_string()),
            thinking_signature: Some("sig".to_string()),
            usage: Some(Usage::new(1, 2, 3)),
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "ai");
        assert_eq!(json["thinking"], "considering");
        assert_eq!(json["usage"]["total_tokens"], 3);
        let restored = BaseMessage::from_dict(&json);
        assert!(matches!(restored, BaseMessage::AIMessage { .. }));
    }
}
