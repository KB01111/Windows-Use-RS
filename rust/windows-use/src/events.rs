//! Agent events, mirroring `windows_use/agent/events`.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Local;
use serde_json::{json, Map, Value};

/// Event kinds emitted by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Thought,
    ToolCall,
    ToolResult,
    Done,
    Error,
}

impl EventType {
    pub fn value(&self) -> &'static str {
        match self {
            EventType::Thought => "thought",
            EventType::ToolCall => "tool_call",
            EventType::ToolResult => "tool_result",
            EventType::Done => "done",
            EventType::Error => "error",
        }
    }
}

/// A single agent event.
#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub event_type: EventType,
    pub data: Map<String, Value>,
}

impl AgentEvent {
    pub fn new(event_type: EventType, data: Map<String, Value>) -> Self {
        Self { event_type, data }
    }

    pub fn thought(step: usize, thought: &str) -> Self {
        let mut data = Map::new();
        data.insert("step".to_string(), json!(step));
        data.insert("thought".to_string(), json!(thought));
        Self::new(EventType::Thought, data)
    }

    pub fn tool_call(step: usize, tool_name: &str, params: &Map<String, Value>) -> Self {
        let mut data = Map::new();
        data.insert("step".to_string(), json!(step));
        data.insert("tool_name".to_string(), json!(tool_name));
        data.insert("tool_params".to_string(), Value::Object(params.clone()));
        Self::new(EventType::ToolCall, data)
    }

    pub fn tool_result(step: usize, tool_name: &str, is_success: bool, content: &str) -> Self {
        let mut data = Map::new();
        data.insert("step".to_string(), json!(step));
        data.insert("tool_name".to_string(), json!(tool_name));
        data.insert("is_success".to_string(), json!(is_success));
        data.insert("content".to_string(), json!(content));
        Self::new(EventType::ToolResult, data)
    }

    pub fn done(step: usize, content: &str) -> Self {
        let mut data = Map::new();
        data.insert("step".to_string(), json!(step));
        data.insert("content".to_string(), json!(content));
        Self::new(EventType::Done, data)
    }

    pub fn error(step: usize, error: &str) -> Self {
        let mut data = Map::new();
        data.insert("step".to_string(), json!(step));
        data.insert("error".to_string(), json!(error));
        Self::new(EventType::Error, data)
    }

    pub fn step(&self) -> usize {
        self.data
            .get("step")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
    }

    pub fn text(&self, key: &str) -> String {
        self.data
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// A subscriber receives every emitted event.
pub trait EventSubscriber: Send + Sync {
    fn invoke(&self, event: &AgentEvent);
    fn close(&self) {}
}

/// Formats `click_tool` as `Click`, `multi_select_tool` as `Multi Select`.
pub fn format_tool_name(tool_name: &str) -> String {
    if tool_name.is_empty() {
        return String::new();
    }
    let name = tool_name.strip_suffix("_tool").unwrap_or(tool_name);
    name.split('_')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Dispatch target accepting both object subscribers and plain closures.
pub enum Subscriber {
    Object(Box<dyn EventSubscriber>),
    Callback(Arc<dyn Fn(&AgentEvent) + Send + Sync>),
}

/// Dispatches events to registered subscribers, swallowing subscriber errors.
#[derive(Default)]
pub struct Event {
    subscribers: Vec<Subscriber>,
}

impl Event {
    pub fn new() -> Self {
        Self { subscribers: Vec::new() }
    }

    pub fn add_subscriber(&mut self, subscriber: Box<dyn EventSubscriber>) {
        self.subscribers.push(Subscriber::Object(subscriber));
    }

    pub fn add_callback(&mut self, callback: Arc<dyn Fn(&AgentEvent) + Send + Sync>) {
        self.subscribers.push(Subscriber::Callback(callback));
    }

    pub fn emit(&self, event: AgentEvent) {
        for subscriber in &self.subscribers {
            match subscriber {
                Subscriber::Object(object) => {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        object.invoke(&event)
                    }));
                }
                Subscriber::Callback(callback) => {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        callback(&event)
                    }));
                }
            }
        }
    }

    pub fn close(&mut self) {
        for subscriber in &self.subscribers {
            if let Subscriber::Object(object) = subscriber {
                object.close();
            }
        }
        self.subscribers.clear();
    }
}

/// Logs agent events through the standard logger, like `ConsoleEventSubscriber`.
pub struct ConsoleEventSubscriber;

impl EventSubscriber for ConsoleEventSubscriber {
    fn invoke(&self, event: &AgentEvent) {
        match event.event_type {
            EventType::Thought => {
                log_info(&format!("[Agent] 🧠 Thinking: {}", event.text("thought")));
            }
            EventType::ToolCall => {
                let name = format_tool_name(&event.text("tool_name"));
                let params = render_params(event.data.get("tool_params"));
                log_info(&format!("[Agent] 🛠️ Tool Call: {name}({params})"));
            }
            EventType::ToolResult => {
                let name = format_tool_name(&event.text("tool_name"));
                let success = event
                    .data
                    .get("is_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let content = event.text("content");
                if !success {
                    log_warning(&format!("[Agent] 🚨 Tool '{name}' failed: {content}"));
                } else if name != "done_tool" {
                    log_info(&format!("[Agent] 📃 Tool Result: {content}"));
                }
            }
            EventType::Done => {
                log_info(&format!("[Agent] 📜 Final Answer: {}", event.text("content")));
            }
            EventType::Error => {
                log_error(&format!("[Agent] 🚨 Error: {}", event.text("error")));
            }
        }
    }
}

fn render_params(value: Option<&Value>) -> String {
    let Some(Value::Object(map)) = value else {
        return String::new();
    };
    map.iter()
        .map(|(key, value)| format!("{key}={}", render_value(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Writes agent events to a timestamped log file, like `FileEventSubscriber`.
pub struct FileEventSubscriber {
    file: Mutex<File>,
}

impl FileEventSubscriber {
    pub fn new(log_path: Option<PathBuf>) -> std::io::Result<Self> {
        let path = match log_path {
            Some(path) => path,
            None => {
                let directory = crate::win32::home_dir().join(".windows-use").join("logs");
                std::fs::create_dir_all(&directory)?;
                directory.join(format!("{}.log", Local::now().format("%Y-%m-%d_%H-%M-%S")))
            }
        };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self { file: Mutex::new(file) })
    }

    fn write(&self, timestamp: &str, message: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "[{timestamp}] {message}");
            let _ = file.flush();
        }
    }
}

impl EventSubscriber for FileEventSubscriber {
    fn invoke(&self, event: &AgentEvent) {
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        match event.event_type {
            EventType::Thought => self.write(&timestamp, &format!("Thought: {}", event.text("thought"))),
            EventType::ToolCall => {
                let name = format_tool_name(&event.text("tool_name"));
                let params = render_params(event.data.get("tool_params"));
                self.write(&timestamp, &format!("Tool Call: {name}({params})"));
            }
            EventType::ToolResult => {
                let raw_name = event.text("tool_name");
                if raw_name != "done_tool" {
                    let success = event
                        .data
                        .get("is_success")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    let status = if success { "Success" } else { "Failed" };
                    self.write(
                        &timestamp,
                        &format!(
                            "Tool Result [{status}]: {} -> {}",
                            format_tool_name(&raw_name),
                            event.text("content")
                        ),
                    );
                }
            }
            EventType::Done => self.write(&timestamp, &format!("Final Answer: {}", event.text("content"))),
            EventType::Error => self.write(&timestamp, &format!("Error: {}", event.text("error"))),
        }
    }
}

/// Minimal logger matching the `[LEVEL] message` format on stderr.
pub fn log_info(message: &str) {
    eprintln!("[INFO] {message}");
}

pub fn log_warning(message: &str) {
    eprintln!("[WARNING] {message}");
}

pub fn log_error(message: &str) {
    eprintln!("[ERROR] {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_title_cased() {
        assert_eq!(format_tool_name("click_tool"), "Click");
        assert_eq!(format_tool_name("multi_select_tool"), "Multi Select");
        assert_eq!(format_tool_name("done_tool"), "Done");
        assert_eq!(format_tool_name("not_a_suffix"), "Not A Suffix");
        assert_eq!(format_tool_name(""), "");
    }

    #[test]
    fn events_carry_expected_payloads() {
        let thought = AgentEvent::thought(3, "thinking");
        assert_eq!(thought.event_type, EventType::Thought);
        assert_eq!(thought.step(), 3);
        assert_eq!(thought.text("thought"), "thinking");

        let done = AgentEvent::done(5, "answer");
        assert_eq!(done.event_type, EventType::Done);
        assert_eq!(done.text("content"), "answer");
    }

    #[test]
    fn subscribers_receive_events() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicUsize::new(0));
        let cloned = counter.clone();
        let mut event = Event::new();
        event.add_callback(Arc::new(move |_| {
            cloned.fetch_add(1, Ordering::SeqCst);
        }));
        event.emit(AgentEvent::thought(0, "x"));
        event.emit(AgentEvent::done(1, "y"));
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn params_render_like_python() {
        let mut params = Map::new();
        params.insert("x".to_string(), json!(10));
        params.insert("text".to_string(), json!("hello"));
        assert_eq!(render_params(Some(&Value::Object(params))), "x=10, text=hello");
    }
}
