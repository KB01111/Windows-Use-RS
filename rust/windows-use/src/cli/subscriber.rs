//! CLI event subscriber, mirroring `windows_use/cli/subscriber.py`.

use std::io::Write;
use std::sync::Mutex;

use crate::events::{format_tool_name, AgentEvent, EventSubscriber, EventType};

/// Truncates long text with an ellipsis, like the Python helper.
pub fn truncate(text: &str, max_len: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= max_len {
        return text.to_string();
    }
    let keep = max_len.saturating_sub(3);
    format!("{}...", characters[..keep].iter().collect::<String>())
}

/// Per-step console output used by the interactive CLI.
pub struct CliEventSubscriber {
    pending_tool_name: Mutex<Option<String>>,
    pending_tool_params: Mutex<Option<String>>,
    tts_callback: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl CliEventSubscriber {
    pub fn new(tts_callback: Option<Box<dyn Fn(&str) + Send + Sync>>) -> Self {
        Self {
            pending_tool_name: Mutex::new(None),
            pending_tool_params: Mutex::new(None),
            tts_callback,
        }
    }
}

/// Strips markdown formatting so the text reads cleanly through TTS.
pub fn strip_markdown(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut output = text.to_string();
    for pattern in ["**", "__", "```"] {
        output = output.replace(pattern, "");
    }
    // Inline code and emphasis markers.
    output = output.replace('`', "");

    let mut cleaned = String::with_capacity(output.len());
    let mut characters = output.chars().peekable();
    let mut at_line_start = true;
    while let Some(character) = characters.next() {
        match character {
            '#' if at_line_start => {
                // Skip the whole run of '#' plus the following space.
                while matches!(characters.peek(), Some('#')) {
                    characters.next();
                }
                if matches!(characters.peek(), Some(' ')) {
                    characters.next();
                }
                at_line_start = false;
            }
            '\n' => {
                cleaned.push('\n');
                at_line_start = true;
            }
            '*' | '_' => {}
            other => {
                cleaned.push(other);
                at_line_start = false;
            }
        }
    }

    // Collapse runs of blank lines, matching the Python `\n{3,}` replacement.
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }
    cleaned.trim().to_string()
}

fn write_line(text: &str) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

impl EventSubscriber for CliEventSubscriber {
    fn invoke(&self, event: &AgentEvent) {
        match event.event_type {
            EventType::Thought => {
                let thought = event.text("thought");
                if !thought.is_empty() {
                    write_line(&format!("Thinking: {}", truncate(&thought, 1_000)));
                }
            }
            EventType::ToolCall => {
                let name = format_tool_name(&event.text("tool_name"));
                let params = match event.data.get("tool_params") {
                    Some(serde_json::Value::Object(map)) => map
                        .iter()
                        .map(|(key, value)| {
                            let rendered = match value {
                                serde_json::Value::String(text) => text.clone(),
                                other => other.to_string(),
                            };
                            format!("{key}={}", truncate(&rendered, 1_000))
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => String::new(),
                };
                if let Ok(mut slot) = self.pending_tool_name.lock() {
                    *slot = Some(name);
                }
                if let Ok(mut slot) = self.pending_tool_params.lock() {
                    *slot = if params.is_empty() { None } else { Some(params) };
                }
            }
            EventType::ToolResult => {
                let name = match self.pending_tool_name.lock().ok().and_then(|mut slot| slot.take()) {
                    Some(name) => name,
                    None => return,
                };
                let params = self
                    .pending_tool_params
                    .lock()
                    .ok()
                    .and_then(|mut slot| slot.take());
                if event.text("tool_name") == "done_tool" {
                    return;
                }
                let success = event
                    .data
                    .get("is_success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                let mark = if success { "✓" } else { "✗" };
                let params_part = params.map(|value| format!("({value})")).unwrap_or_else(|| "()".to_string());
                write_line(&format!("Tool: {name}{params_part} {mark}"));
            }
            EventType::Done => {
                let content = event.text("content").trim().to_string();
                let answer = if content.is_empty() {
                    "No answer provided.".to_string()
                } else {
                    content
                };
                write_line(&format!("Answer: {answer}"));
                if answer != "No answer provided." {
                    if let Some(callback) = &self.tts_callback {
                        callback(&answer);
                    }
                }
            }
            EventType::Error => {
                let error = event.text("error");
                write_line(&format!("Error: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_matches_python() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn markdown_is_stripped_for_speech() {
        assert_eq!(strip_markdown("**bold** and `code`"), "bold and code");
        assert_eq!(strip_markdown("# Heading\ntext"), "Heading\ntext");
        assert_eq!(strip_markdown("a\n\n\n\nb"), "a\n\nb");
        assert_eq!(strip_markdown("* item"), "item");
        assert_eq!(strip_markdown(""), "");
    }
}
