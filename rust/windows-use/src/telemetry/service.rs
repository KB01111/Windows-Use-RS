//! PostHog product telemetry, mirroring `windows_use/telemetry/service.py`.
//!
//! The Python implementation wraps the `posthog` SDK; here the batch payload is
//! posted directly with `reqwest`. Two rules from the original carry over:
//! telemetry is enabled unless `ANONYMIZED_TELEMETRY=false`, and every failure is
//! swallowed so a network problem can never surface to the user.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::telemetry::views::TelemetryEvent;
use crate::VERSION;

/// PostHog project API key, copied verbatim from the Python service.
pub const PROJECT_API_KEY: &str = "phc_uxdCItyVTjXNU0sMPr97dq3tcz39scQNt3qjTYw5vLV";
/// PostHog host, copied verbatim from the Python service.
pub const HOST: &str = "https://us.i.posthog.com";
/// Batch endpoint used to send one event at a time.
pub const BATCH_ENDPOINT: &str = "https://us.i.posthog.com/batch/";
/// Directory (inside the temp dir) holding the persisted anonymous user id.
pub const TELEMETRY_DIR_NAME: &str = ".windows-use";
/// File name of the persisted anonymous user id.
pub const USER_ID_FILE_NAME: &str = ".windows-use-user-id";
/// Environment variable that turns telemetry off when set to `false`.
pub const TELEMETRY_ENV_VAR: &str = "ANONYMIZED_TELEMETRY";

/// PostHog product telemetry client.
///
/// Construct with [`ProductTelemetry::new`]; `new` reads the environment once, the
/// same way the Python `__init__` does. Failures are recorded as `None` and never
/// propagated, and `capture` never returns an error.
///
/// The crate's `reqwest` is built without the `blocking` feature, so POSTs use the
/// async client driven by a private current-thread runtime. `capture` hands the
/// request to a worker thread and returns immediately, which keeps the agent's hot
/// path free of network latency.
pub struct ProductTelemetry {
    enabled: bool,
    user_id: String,
    /// The HTTP client, created lazily only when telemetry is enabled.
    client: Mutex<Option<reqwest::Client>>,
    /// Runtime that drives the client, created alongside it.
    runtime: Mutex<Option<Arc<tokio::runtime::Runtime>>>,
}

impl std::fmt::Debug for ProductTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductTelemetry")
            .field("enabled", &self.enabled)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl Default for ProductTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProductTelemetry {
    /// Creates the client, reading `ANONYMIZED_TELEMETRY` the way Python does.
    ///
    /// The variable defaults to enabled; only the literal value `false`
    /// (case-insensitive) disables it.
    pub fn new() -> Self {
        Self {
            enabled: Self::telemetry_enabled_from_env(),
            user_id: Self::load_or_create_user_id(),
            client: Mutex::new(None),
            runtime: Mutex::new(None),
        }
    }

    /// Creates a disabled client that never performs network calls.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            user_id: Self::load_or_create_user_id(),
            client: Mutex::new(None),
            runtime: Mutex::new(None),
        }
    }

    /// Creates a client with an explicit enable flag, for tests and embedders.
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            user_id: Self::load_or_create_user_id(),
            client: Mutex::new(None),
            runtime: Mutex::new(None),
        }
    }

    /// Reads `ANONYMIZED_TELEMETRY`, defaulting to enabled.
    fn telemetry_enabled_from_env() -> bool {
        match std::env::var(TELEMETRY_ENV_VAR) {
            Ok(value) => !value.trim().eq_ignore_ascii_case("false"),
            Err(_) => true,
        }
    }

    /// Whether telemetry will actually send events.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The stable anonymous user id used as PostHog `distinct_id`.
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The directory holding the persisted user id.
    pub fn user_id_dir() -> PathBuf {
        std::env::temp_dir().join(TELEMETRY_DIR_NAME)
    }

    /// The file holding the persisted user id.
    pub fn user_id_path() -> PathBuf {
        Self::user_id_dir().join(USER_ID_FILE_NAME)
    }

    /// Loads the persisted user id, generating and storing one when absent.
    ///
    /// Mirrors the Python `user_id` property: any I/O failure is ignored and an
    /// in-memory id is used instead.
    pub fn load_or_create_user_id() -> String {
        Self::load_or_create_user_id_in(&Self::user_id_dir())
    }

    /// [`ProductTelemetry::load_or_create_user_id`] against an explicit directory.
    pub fn load_or_create_user_id_in(dir: &Path) -> String {
        let path = dir.join(USER_ID_FILE_NAME);
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        let user_id = generate_user_id();
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(&path, &user_id);
        user_id
    }

    /// Captures one event, sending it to PostHog on a background thread.
    ///
    /// Returns immediately: the request runs on a worker so agent latency is
    /// unaffected, and every error is dropped silently like in Python.
    pub fn capture(&self, event: &dyn TelemetryEvent) {
        if !self.enabled {
            return;
        }

        let payload = self.build_payload(event);
        let Some((client, runtime)) = self.http() else {
            return;
        };

        // A detached thread keeps `capture` non-blocking without requiring a Tokio
        // runtime to be present in the caller.
        let _ = std::thread::Builder::new()
            .name("windows-use-telemetry".to_string())
            .spawn(move || {
                let request = client.post(BATCH_ENDPOINT).json(&payload).send();
                let _ = runtime.block_on(request);
            });
    }

    /// Captures one event and waits for the HTTP call to finish.
    ///
    /// Used by tests and by short-lived processes that must not lose the event;
    /// failures are still swallowed and reported as `false`.
    pub fn capture_blocking(&self, event: &dyn TelemetryEvent) -> bool {
        if !self.enabled {
            return false;
        }
        let payload = self.build_payload(event);
        let Some((client, runtime)) = self.http() else {
            return false;
        };
        runtime
            .block_on(client.post(BATCH_ENDPOINT).json(&payload).send())
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    }

    /// Builds the PostHog batch body for one event.
    ///
    /// Shape matches the Python `client.capture` call:
    /// `{"api_key": ..., "event": ..., "properties": {<event>, "distinct_id": ...,
    /// "process_person_profile": true}, "timestamp": ...}`.
    pub fn build_payload(&self, event: &dyn TelemetryEvent) -> Value {
        let mut properties: Map<String, Value> = event.properties();
        properties.insert("distinct_id".to_string(), json!(self.user_id));
        properties.insert("process_person_profile".to_string(), json!(true));

        json!({
            "api_key": PROJECT_API_KEY,
            "event": event.event_name(),
            "properties": Value::Object(properties),
            "timestamp": timestamp_now(),
        })
    }

    /// Lazily builds the HTTP client and its runtime, returning `None` on failure.
    ///
    /// The client is created once and shared; the runtime is reference counted so
    /// background workers and synchronous callers can borrow it cheaply.
    fn http(&self) -> Option<(reqwest::Client, Arc<tokio::runtime::Runtime>)> {
        let runtime = {
            let mut guard = self.runtime.lock().ok()?;
            match guard.as_ref() {
                Some(runtime) => Arc::clone(runtime),
                None => {
                    let runtime = Arc::new(
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .ok()?,
                    );
                    *guard = Some(Arc::clone(&runtime));
                    runtime
                }
            }
        };

        let mut guard = self.client.lock().ok()?;
        let client = match guard.as_ref() {
            Some(client) => client.clone(),
            None => {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .ok()?;
                *guard = Some(client.clone());
                client
            }
        };

        Some((client, runtime))
    }

    /// Whether the PostHog client has been built (used by the tests).
    pub fn has_client(&self) -> bool {
        self.client
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Flushes pending events.
    ///
    /// The Python client queues events and joins on exit; here each capture is sent
    /// from its own thread, so there is nothing to drain and this is a no-op that
    /// matches the original signature.
    pub fn flush(&self) {}

    /// The crate version reported alongside events, matching the Python `version`.
    pub fn version() -> &'static str {
        VERSION
    }
}

/// Current UNIX time in seconds with fractional milliseconds, like PostHog's
/// `timestamp` field.
fn timestamp_now() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(_) => 0.0,
    }
}

/// Generates a time-ordered, UUIDv7-shaped identifier.
///
/// Python uses `uuid_extensions.uuid7str()`; the layout here is the same – the
/// first 48 bits are the UNIX time in milliseconds – so ids sort by creation and
/// remain unique through the random tail.
pub fn generate_user_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);

    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..8]);

    // 16 random bytes supply the `rand_a`/`rand_b` halves of a UUIDv7.
    let random = uuid::Uuid::new_v4().into_bytes();
    bytes[6..].copy_from_slice(&random[6..]);

    // Set the version (7) and RFC 4122 variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    uuid::Uuid::from_bytes(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::views::AgentTelemetryEvent;

    #[test]
    fn constants_match_the_python_service() {
        assert_eq!(PROJECT_API_KEY, "phc_uxdCItyVTjXNU0sMPr97dq3tcz39scQNt3qjTYw5vLV");
        assert_eq!(HOST, "https://us.i.posthog.com");
        assert_eq!(BATCH_ENDPOINT, "https://us.i.posthog.com/batch/");
    }

    #[test]
    fn generated_user_ids_are_unique_and_uuidv7_shaped() {
        let first = generate_user_id();
        let second = generate_user_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);

        let parsed = uuid::Uuid::parse_str(&first).expect("valid uuid");
        assert_eq!(parsed.get_version_num(), 7);
        assert_eq!(first.as_bytes()[14], b'7');

        // The leading bytes carry the millisecond timestamp, so they sort by time.
        let first_bytes = uuid::Uuid::parse_str(&first).expect("uuid").into_bytes();
        let second_bytes = uuid::Uuid::parse_str(&second).expect("uuid").into_bytes();
        assert!(first_bytes[..6] <= second_bytes[..6]);
    }

    #[test]
    fn user_id_file_is_created_and_reused() {
        let dir = std::env::temp_dir().join(format!("windows-use-telem-{}", uuid::Uuid::new_v4()));

        let created = ProductTelemetry::load_or_create_user_id_in(&dir);
        assert!(!created.is_empty());

        let file = dir.join(USER_ID_FILE_NAME);
        assert!(file.exists(), "user id file should be written");

        let reused = ProductTelemetry::load_or_create_user_id_in(&dir);
        assert_eq!(created, reused);

        // A blank file is treated as missing, matching the Python truthiness check.
        std::fs::write(&file, "   ").expect("blank the file");
        let regenerated = ProductTelemetry::load_or_create_user_id_in(&dir);
        assert!(!regenerated.trim().is_empty());
        assert_ne!(regenerated, "   ");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_telemetry_never_sends() {
        let client = ProductTelemetry::with_enabled(false);
        assert!(!client.is_enabled());
        let event = AgentTelemetryEvent::new("q", 1, 2, "m", "p");
        assert!(!client.capture_blocking(&event));
        // A disabled client must not even build an HTTP client.
        assert!(!client.has_client());
        client.flush();
    }

    #[test]
    fn env_var_disables_telemetry_only_for_false() {
        // The helper is private; mirror its rule through the public constructor.
        let key = TELEMETRY_ENV_VAR;
        let previous = std::env::var(key).ok();

        std::env::set_var(key, "false");
        assert!(!ProductTelemetry::new().is_enabled());

        std::env::set_var(key, "FALSE");
        assert!(!ProductTelemetry::new().is_enabled());

        std::env::set_var(key, "true");
        assert!(ProductTelemetry::new().is_enabled());

        std::env::remove_var(key);
        assert!(ProductTelemetry::new().is_enabled());

        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn payload_has_the_posthog_batch_shape() {
        let client = ProductTelemetry::with_enabled(true);
        let event = AgentTelemetryEvent::new("open notepad", 3, 10, "gpt-5.2", "openai")
            .with_answer("opened")
            .with_success(true);

        let payload = client.build_payload(&event);
        assert_eq!(payload["api_key"], json!(PROJECT_API_KEY));
        assert_eq!(payload["event"], json!("agent_event"));
        assert!(payload["timestamp"].is_number());

        let properties = &payload["properties"];
        assert_eq!(properties["distinct_id"], json!(client.user_id()));
        assert_eq!(properties["process_person_profile"], json!(true));
        assert_eq!(properties["query"], json!("open notepad"));
        assert_eq!(properties["steps"], json!(3));
        assert_eq!(properties["max_steps"], json!(10));
        assert_eq!(properties["model"], json!("gpt-5.2"));
        assert_eq!(properties["provider"], json!("openai"));
        assert_eq!(properties["answer"], json!("opened"));
        assert_eq!(properties["is_success"], json!(true));
        assert_eq!(properties["event_name"], json!("agent_event"));
    }

    #[test]
    fn payload_serializes_to_valid_json() {
        let client = ProductTelemetry::with_enabled(true);
        let event = AgentTelemetryEvent::new("q", 0, 1, "m", "p");
        let payload = client.build_payload(&event);
        let text = serde_json::to_string(&payload).expect("serialize");
        let parsed: Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed["event"], json!("agent_event"));
    }

    #[test]
    fn user_id_paths_live_under_the_temp_dir() {
        assert!(ProductTelemetry::user_id_dir().starts_with(std::env::temp_dir()));
        assert!(ProductTelemetry::user_id_dir().ends_with(TELEMETRY_DIR_NAME));
        assert!(ProductTelemetry::user_id_path().ends_with(USER_ID_FILE_NAME));
    }

    #[test]
    fn flush_is_a_noop_that_succeeds() {
        let client = ProductTelemetry::with_enabled(false);
        client.flush();
        assert!(!client.is_enabled());
    }

    #[test]
    fn version_matches_the_crate_version() {
        assert_eq!(ProductTelemetry::version(), VERSION);
    }
}
