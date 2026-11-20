//! Provider traits and the shared HTTP machinery, mirroring
//! `windows_use/providers/base.py`.
//!
//! The Python port uses `Protocol` classes. Here they are async traits written
//! with explicit boxed futures so implementors need no extra crate.
//!
//! Every provider is constructible without network access and without an API
//! key: missing credentials surface as errors from `invoke`, never as panics.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::Value;

use crate::messages::BaseMessage;
use crate::providers::events::{LLMEvent, LLMStreamEvent};
use crate::providers::tools::ToolSchema;
use crate::providers::views::Metadata;

/// Boxed future used by the object-safe async traits in this module.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stream of incremental events returned by `stream`/`astream`.
pub type EventStream<'a> = BoxStream<'a, Result<LLMStreamEvent>>;

/// The LLM protocol every chat provider implements.
///
/// `invoke` and `stream` take `&self` and return boxed futures so the trait stays
/// object safe (`Box<dyn ChatLlm>`). Providers that support Python's `json_mode`
/// flag expose it as an inherent `invoke_json`/`stream_json` method, keeping this
/// trait to the two-argument shape the agent loop needs.
pub trait ChatLlm: Send + Sync {
    /// The model identifier passed to the API.
    fn model_name(&self) -> &str;

    /// The provider id, e.g. `"openai"` or `"azure_openai"`.
    fn provider(&self) -> &str;

    /// Context-window metadata for the configured model.
    fn get_metadata(&self) -> Metadata;

    /// Non-streaming call.
    fn invoke<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<LLMEvent>>;

    /// Streaming call; the default adapts [`ChatLlm::invoke`] into a three-event
    /// text stream so every provider supports streaming.
    fn stream<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<EventStream<'a>>> {
        Box::pin(async move {
            let event = self.invoke(messages, tools).await?;
            Ok(single_event_stream(event))
        })
    }

    /// Async alias of [`ChatLlm::invoke`], matching the Python `ainvoke` name.
    fn ainvoke<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<LLMEvent>> {
        self.invoke(messages, tools)
    }

    /// Async alias of [`ChatLlm::stream`], matching the Python `astream` name.
    fn astream<'a>(
        &'a self,
        messages: &'a [BaseMessage],
        tools: &'a [ToolSchema],
    ) -> BoxFuture<'a, Result<EventStream<'a>>> {
        self.stream(messages, tools)
    }
}

/// The Python protocol's name, kept as an alias for readers of the original.
pub use ChatLlm as BaseChatLlm;

/// Turns a completed [`LLMEvent`] into the equivalent one-shot stream.
pub fn single_event_stream(event: LLMEvent) -> EventStream<'static> {
    let events: Vec<Result<LLMStreamEvent>> = if event.is_tool_call() {
        vec![Ok(LLMStreamEvent::new(
            crate::providers::events::LLMStreamEventType::ToolCall,
        )
        .with_tool_call(event.tool_call.clone().unwrap_or_default())
        .with_thinking(event.thinking.clone())
        .with_usage(Some(event.usage)))]
    } else {
        let mut produced = Vec::new();
        produced.push(Ok(LLMStreamEvent::text_start()));
        if let Some(content) = event.content.as_deref() {
            if !content.is_empty() {
                produced.push(Ok(LLMStreamEvent::text_delta(content)));
            }
        }
        produced.push(Ok(LLMStreamEvent::text_end(Some(event.usage))));
        produced
    };
    futures_util::stream::iter(events).boxed()
}

/// Speech-to-text protocol, mirroring `BaseSTT`.
pub trait SttProvider: Send + Sync {
    /// The STT model name.
    fn model(&self) -> &str;

    /// Transcribes an audio file on disk.
    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>>;
}

/// Text-to-speech protocol, mirroring `BaseTTS`.
pub trait TtsProvider: Send + Sync {
    /// The TTS model name.
    fn model(&self) -> &str;

    /// Writes synthesized speech for `text` to `output_path`.
    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
}

/// Reads an environment variable, treating empty values as unset (Python's
/// `os.environ.get` returns `""` but every provider guards on truthiness).
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// First non-empty value among `explicit` and the given environment variables.
pub fn resolve_setting(explicit: Option<String>, env_names: &[&str]) -> Option<String> {
    if let Some(value) = explicit.filter(|value| !value.is_empty()) {
        return Some(value);
    }
    env_names.iter().find_map(|name| env_var(name))
}

/// Error describing a provider that has no usable API key yet.
pub fn missing_api_key(provider: &str, env_names: &[&str]) -> anyhow::Error {
    anyhow!(
        "{provider} API key is not set; pass it to the constructor or set {}",
        env_names.join(" or ")
    )
}

/// Requires an API key, mirroring the Python clients that fail on first use.
pub fn require_api_key(api_key: &Option<String>, provider: &str, env_names: &[&str]) -> Result<()> {
    match api_key {
        Some(key) if !key.is_empty() => Ok(()),
        _ => Err(missing_api_key(provider, env_names)),
    }
}

/// HTTP transport shared by the providers that talk REST directly.
///
/// Wraps a `reqwest::Client` with the configured timeout and retry count and
/// exposes the JSON POST / streaming POST helpers plus SSE line decoding.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    timeout: Duration,
    max_retries: u32,
}

impl HttpTransport {
    /// Builds a transport. `timeout` is in seconds, matching the Python clients.
    pub fn new(timeout_seconds: f64, max_retries: u32) -> Self {
        let timeout = Duration::from_secs_f64(if timeout_seconds.is_finite() && timeout_seconds > 0.0
        {
            timeout_seconds
        } else {
            600.0
        });
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            timeout,
            max_retries,
        }
    }

    /// The configured per-request timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The configured retry budget.
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Borrows the underlying client for provider-specific calls.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// POSTs JSON and decodes the JSON reply, retrying transient failures.
    pub async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
    ) -> Result<Value> {
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..=self.max_retries {
            let mut request = self.client.post(url).json(body);
            for (name, value) in headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.send().await {
                Ok(response) => match decode_json_response(response).await {
                    Ok(value) => return Ok(value),
                    Err(error) => {
                        if !is_retryable_message(&error) {
                            return Err(error);
                        }
                        last_error = Some(error);
                    }
                },
                Err(error) => {
                    if attempt == self.max_retries || !is_retryable_reqwest(&error) {
                        return Err(anyhow::Error::new(error).context(format!("POST {url}")));
                    }
                    last_error = Some(anyhow::Error::new(error).context(format!("POST {url}")));
                }
            }
            tokio::time::sleep(backoff(attempt)).await;
        }
        Err(last_error.unwrap_or_else(|| anyhow!("POST {url} failed")))
    }

    /// POSTs JSON and returns the raw byte stream of the response body.
    pub async fn post_stream(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &Value,
    ) -> Result<reqwest::Response> {
        let mut request = self.client.post(url).json(body);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("POST {url} failed with {status}: {}", truncate(&body, 800)));
        }
        Ok(response)
    }
}

/// Decodes a JSON body, turning non-2xx replies into descriptive errors.
pub async fn decode_json_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .context("reading response body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "request failed with {status}: {}",
            truncate(&body, 800)
        ));
    }
    serde_json::from_str(&body)
        .with_context(|| format!("decoding response JSON: {}", truncate(&body, 400)))
}

/// Recursively pulls the first `text` field out of a JSON error body so provider
/// error messages stay readable.
pub fn error_message_from_body(body: &Value) -> Option<String> {
    if let Some(message) = body.get("error").and_then(|error| {
        error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
    }) {
        return Some(message.to_string());
    }
    body.get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| body.get("detail").and_then(Value::as_str).map(str::to_string))
}

fn is_retryable_reqwest(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_request()
}

fn is_retryable_message(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("429")
        || text.contains("500")
        || text.contains("502")
        || text.contains("503")
        || text.contains("504")
        || text.contains("timed out")
}

fn backoff(attempt: u32) -> Duration {
    let milliseconds = 250u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(milliseconds)
}

/// Truncates long provider payloads for error messages.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}...")
}

/// Incremental decoder for Server-Sent Events bodies.
///
/// Feeds raw byte chunks in and yields the `data:` payload of each event; the
/// `[DONE]` sentinel is reported separately so callers can stop cleanly.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a byte chunk and returns the `data:` payloads it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut payloads = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() {
                    payloads.push(data.to_string());
                }
            }
        }
        payloads
    }

    /// Flushes a trailing partial line (servers sometimes omit the final newline).
    pub fn finish(&mut self) -> Vec<String> {
        let remainder = std::mem::take(&mut self.buffer);
        let line = remainder.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() {
                return vec![data.to_string()];
            }
        }
        Vec::new()
    }
}

/// Parses one SSE payload into JSON, ignoring the sentinel.
pub fn parse_sse_json(payload: &str) -> Option<Value> {
    if payload.trim() == "[DONE]" {
        return None;
    }
    serde_json::from_str(payload).ok()
}

/// Framing of a streaming response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFraming {
    /// `data: {...}` server-sent events (OpenAI, Anthropic, Google).
    Sse,
    /// One JSON document per line (Ollama).
    Lines,
}

/// Splits a byte stream into newline-delimited lines.
#[derive(Debug, Default)]
pub struct LineDecoder {
    buffer: String,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes in and returns the complete non-empty lines observed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut lines = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        lines
    }
    /// Flushes a trailing partial line.
    pub fn finish(&mut self) -> Vec<String> {
        let remainder = std::mem::take(&mut self.buffer);
        let line = remainder.trim();
        if line.is_empty() {
            Vec::new()
        } else {
            vec![line.to_string()]
        }
    }
}

/// Drives a streaming JSON response into an [`EventStream`].
///
/// `on_payload` turns each decoded JSON document into zero or more events and
/// `on_end` emits the closing events (text/tool-call finalisation) once the body
/// is exhausted. One implementation of the plumbing serves every provider.
pub fn json_event_stream<S, H, E>(
    response: reqwest::Response,
    framing: StreamFraming,
    initial_state: S,
    on_payload: H,
    on_end: E,
) -> EventStream<'static>
where
    S: Send + 'static,
    H: FnMut(&mut S, Value) -> Vec<LLMStreamEvent> + Send + 'static,
    E: FnMut(&mut S) -> Vec<LLMStreamEvent> + Send + 'static,
{
    struct Driver<S, H, E> {
        state: S,
        on_payload: H,
        on_end: E,
        framing: StreamFraming,
        response: reqwest::Response,
        sse: SseDecoder,
        lines: LineDecoder,
        queue: std::collections::VecDeque<LLMStreamEvent>,
        finished: bool,
    }

    let driver = Driver {
        state: initial_state,
        on_payload,
        on_end,
        framing,
        response,
        sse: SseDecoder::new(),
        lines: LineDecoder::new(),
        queue: std::collections::VecDeque::new(),
        finished: false,
    };

    futures_util::stream::unfold(driver, |mut driver| async move {
        loop {
            if let Some(event) = driver.queue.pop_front() {
                return Some((Ok(event), driver));
            }
            if driver.finished {
                return None;
            }
            // `Response::chunk` keeps `bytes::Bytes` out of this module's API.
            match driver.response.chunk().await {
                Ok(Some(chunk)) => {
                    let payloads = match driver.framing {
                        StreamFraming::Sse => driver.sse.push(&chunk),
                        StreamFraming::Lines => driver.lines.push(&chunk),
                    };
                    for payload in payloads {
                        if let Some(value) = parse_stream_payload(&driver.framing, &payload) {
                            let events = (driver.on_payload)(&mut driver.state, value);
                            driver.queue.extend(events);
                        }
                    }
                }
                Ok(None) => {
                    let trailing = match driver.framing {
                        StreamFraming::Sse => driver.sse.finish(),
                        StreamFraming::Lines => driver.lines.finish(),
                    };
                    for payload in trailing {
                        if let Some(value) = parse_stream_payload(&driver.framing, &payload) {
                            let events = (driver.on_payload)(&mut driver.state, value);
                            driver.queue.extend(events);
                        }
                    }
                    let closing = (driver.on_end)(&mut driver.state);
                    driver.queue.extend(closing);
                    driver.finished = true;
                }
                Err(error) => {
                    driver.finished = true;
                    let error = anyhow::Error::new(error).context("streaming response body");
                    return Some((Err(error), driver));
                }
            }
        }
    })
    .boxed()
}

fn parse_stream_payload(framing: &StreamFraming, payload: &str) -> Option<Value> {
    match framing {
        StreamFraming::Sse => parse_sse_json(payload),
        StreamFraming::Lines => serde_json::from_str(payload).ok(),
    }
}

/// Extracts an `i64` from an optional JSON number/string field.
pub fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Extracts a nested string field with a dotted path, e.g.
/// `"usage.completion_tokens_details.reasoning_tokens"`.
pub fn json_path_i64(value: &Value, path: &str) -> Option<i64> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    json_i64(Some(current))
}

/// Collects the text of a `delta.reasoning_content`-style field, trying each
/// candidate key in order (providers disagree on `reasoning` vs `reasoning_content`).
pub fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_splits_events_across_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"a\"").is_empty());
        let payloads = decoder.push(b":1}\n\ndata: [DONE]\n\n");
        assert_eq!(payloads, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert!(parse_sse_json(&payloads[0]).is_some());
        assert!(parse_sse_json(&payloads[1]).is_none());
    }

    #[test]
    fn sse_decoder_handles_crlf() {
        let mut decoder = SseDecoder::new();
        let payloads = decoder.push(b"event: x\r\ndata: {\"t\":\"hi\"}\r\n\r\n");
        assert_eq!(payloads, vec!["{\"t\":\"hi\"}".to_string()]);
    }

    #[test]
    fn sse_decoder_flushes_trailing_line() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"a\":2}").is_empty());
        assert_eq!(decoder.finish(), vec!["{\"a\":2}".to_string()]);
    }

    #[test]
    fn json_path_helpers_are_tolerant() {
        let value = serde_json::json!({
            "usage": {"completion_tokens_details": {"reasoning_tokens": 42}},
            "n": "17"
        });
        assert_eq!(json_path_i64(&value, "usage.completion_tokens_details.reasoning_tokens"), Some(42));
        assert_eq!(json_path_i64(&value, "usage.missing.path"), None);
        assert_eq!(json_i64(value.get("n")), Some(17));
        assert_eq!(
            first_string_field(&value, &["reasoning", "reasoning_content"]),
            None
        );
    }

    #[test]
    fn resolve_setting_prefers_explicit() {
        let key = "WINDOWS_USE_TEST_RESOLVE_SETTING";
        std::env::remove_var(key);
        assert_eq!(resolve_setting(Some("explicit".into()), &[key]), Some("explicit".to_string()));
        assert_eq!(resolve_setting(None, &[key]), None);
        std::env::set_var(key, "from-env");
        assert_eq!(resolve_setting(None, &[key]), Some("from-env".to_string()));
        std::env::remove_var(key);
    }

    #[test]
    fn missing_key_error_names_the_env_var() {
        let error = require_api_key(&None, "groq", &["GROQ_API_KEY"]).unwrap_err();
        assert!(error.to_string().contains("GROQ_API_KEY"));
        assert!(require_api_key(&Some(String::new()), "groq", &["GROQ_API_KEY"]).is_err());
        assert!(require_api_key(&Some("k".into()), "groq", &["GROQ_API_KEY"]).is_ok());
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 3), "abc...");
    }
}
