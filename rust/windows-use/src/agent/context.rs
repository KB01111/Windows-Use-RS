//! Prompt construction, mirroring `windows_use/agent/context/service.py`.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::desktop::service::Desktop;
use crate::desktop::views::{Browser, DesktopState};
use crate::messages::BaseMessage;

/// Prompt templates, embedded at compile time so the binary is self-contained.
pub mod templates {
    pub const SYSTEM: &str = include_str!("prompts/system.md");
    pub const SYSTEM_FLASH: &str = include_str!("prompts/system_flash.md");
    pub const STATE: &str = include_str!("prompts/state.md");
    pub const COMPACT: &str = include_str!("prompts/compact.md");
}

const NON_TOOL_PARAMS: [&str; 1] = ["thought"];
const COMPACTION_CONTENT_LIMIT: usize = 2000;
const COMPACTION_PARAMS_LIMIT: usize = 4000;
const COMPACTION_THOUGHT_LIMIT: usize = 1000;
const TRUNCATED: &str = "...[TRUNCATED]";

/// Agent mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Flash,
    Normal,
}

impl AgentMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "flash" => Some(AgentMode::Flash),
            "normal" => Some(AgentMode::Normal),
            _ => None,
        }
    }
}

impl Default for AgentMode {
    fn default() -> Self {
        AgentMode::Normal
    }
}

/// Renders JSON values before truncating, preserving their types in history.
fn bounded_text(value: &Value, limit: usize) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > limit {
        let keep = limit.saturating_sub(TRUNCATED.chars().count());
        format!("{}{TRUNCATED}", text.chars().take(keep).collect::<String>())
    } else {
        text
    }
}

/// Python's `str.format` on a template containing `{placeholder}` tokens.
pub fn fill_template(template: &str, values: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut characters = template.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if character == '{' {
            // Escaped braces `{{` produce a literal `{`.
            if matches!(characters.peek(), Some((_, '{'))) {
                characters.next();
                output.push('{');
                continue;
            }
            if let Some(close) = template[index..].find('}') {
                let name = &template[index + 1..index + close];
                if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) {
                    output.push_str(value);
                } else {
                    // Unknown placeholder: keep it verbatim like Python raises KeyError, but
                    // rendering the literal keeps prompts usable.
                    output.push('{');
                    output.push_str(name);
                    output.push('}');
                }
                for _ in 0..close {
                    let _ = characters.next();
                }
                continue;
            }
        } else if character == '}' && matches!(characters.peek(), Some((_, '}'))) {
            characters.next();
            output.push('}');
            continue;
        }
        output.push(character);
    }
    output
}

/// Builds the agent system prompt.
pub fn build_system_prompt(
    mode: AgentMode,
    desktop: &mut Desktop,
    browser: Browser,
    max_steps: usize,
    instructions: &[String],
) -> String {
    let now = chrono::Local::now();
    let datetime = now.format("%A, %B %d, %Y").to_string();

    match mode {
        AgentMode::Flash => {
            let os_version = desktop.get_windows_version();
            fill_template(
                templates::SYSTEM_FLASH,
                &[
                    ("max_steps", max_steps.to_string()),
                    ("datetime", datetime),
                    ("os", os_version),
                    ("browser", browser.value().to_string()),
                ],
            )
        }
        AgentMode::Normal => {
            let os_version = desktop.get_windows_version();
            let language = desktop.get_default_language();
            let user_account_type = desktop.get_user_account_type();
            let (width, height) = crate::uia::virtual_screen_size();
            let home = crate::win32::home_dir();
            let downloads = home.join("Downloads");

            fill_template(
                templates::SYSTEM,
                &[
                    ("datetime", datetime),
                    ("instructions", instructions.join("\n")),
                    (
                        "download_directory",
                        downloads.to_string_lossy().replace('\\', "/"),
                    ),
                    ("os", os_version),
                    ("language", language),
                    ("browser", browser.value().to_string()),
                    ("home_dir", home.to_string_lossy().replace('\\', "/")),
                    (
                        "user",
                        format!(
                            "{} ({})",
                            crate::win32::current_user(),
                            user_account_type
                        ),
                    ),
                    (
                        "resolution",
                        format!(
                            "Primary Monitor ({width}x{height}) with DPI Scale: {}",
                            desktop.get_dpi_scaling()
                        ),
                    ),
                    ("max_steps", max_steps.to_string()),
                ],
            )
        }
    }
}

/// Builds the per-step desktop state prompt.
pub fn build_state_prompt(
    state: &DesktopState,
    query: &str,
    step: usize,
    max_steps: usize,
    use_accessibility: bool,
    nudge: &str,
) -> String {
    let (cursor_x, cursor_y) = crate::uia::get_cursor_pos();
    let loop_warning = if nudge.is_empty() {
        String::new()
    } else {
        format!("[Loop Warning]\n{nudge}\n[End of Loop Warning]\n")
    };

    let interactive_elements = if use_accessibility {
        state
            .tree_state
            .as_ref()
            .map(|tree| tree.interactive_elements_to_string())
            .unwrap_or_default()
    } else {
        "No accessibility data is available".to_string()
    };
    let scrollable_elements = if use_accessibility {
        state
            .tree_state
            .as_ref()
            .map(|tree| tree.scrollable_elements_to_string())
            .unwrap_or_default()
    } else {
        "No accessibility data is available".to_string()
    };

    fill_template(
        templates::STATE,
        &[
            ("steps", step.to_string()),
            ("max_steps", max_steps.to_string()),
            ("loop_warning", loop_warning),
            ("active_window", state.active_window_to_string()),
            ("windows", state.windows_to_string()),
            ("cursor_location", format!("({cursor_x},{cursor_y})")),
            ("interactive_elements", interactive_elements),
            ("scrollable_elements", scrollable_elements),
            ("active_desktop", state.active_desktop_to_string()),
            ("desktops", state.desktops_to_string()),
            ("query", query.to_string()),
        ],
    )
}

/// Formats conversation history for the compaction request.
pub fn format_history_for_compaction(messages: &[BaseMessage]) -> String {
    let mut lines: Vec<String> = vec!["Following is the conversation that needs to be compacted:".to_string()];

    for message in messages {
        match message {
            BaseMessage::SystemMessage { .. } => continue,
            BaseMessage::HumanMessage { content } | BaseMessage::ImageMessage { content, .. } => {
                lines.push(format!(
                    "USER: {}",
                    bounded_text(&Value::String(content.clone()), COMPACTION_CONTENT_LIMIT)
                ));
            }
            BaseMessage::AIMessage { content, .. } => {
                let text = content.clone().unwrap_or_default();
                lines.push(format!(
                    "ASSISTANT: {}",
                    bounded_text(&Value::String(text), COMPACTION_CONTENT_LIMIT)
                ));
            }
            BaseMessage::ToolMessage {
                id,
                name,
                params,
                content,
                ..
            } => {
                if let Some(thought) = params.get("thought") {
                    lines.push(format!(
                        "THOUGHT: {}",
                        bounded_text(thought, COMPACTION_THOUGHT_LIMIT)
                    ));
                }
                let filtered: serde_json::Map<String, Value> = params
                    .iter()
                    .filter(|(key, _)| !NON_TOOL_PARAMS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                lines.push(format!(
                    "TOOL CALL {id}: {name}({})",
                    bounded_text(&Value::Object(filtered), COMPACTION_PARAMS_LIMIT)
                ));
                lines.push(format!(
                    "TOOL RESULT {id}: {}",
                    bounded_text(
                        &Value::String(content.clone().unwrap_or_default()),
                        COMPACTION_CONTENT_LIMIT
                    )
                ));
            }
        }
    }

    lines.join("\n\n---\n\n")
}

/// Builds the two messages sent to the LLM for compaction.
pub fn compaction_messages(messages: &[BaseMessage]) -> Vec<BaseMessage> {
    vec![
        BaseMessage::system(templates::COMPACT),
        BaseMessage::human(format_history_for_compaction(messages)),
    ]
}

/// Wraps a compaction summary in the restoration header.
pub fn format_compaction_response(summary: &str) -> Option<String> {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(
        "\n# Context Restoration (Previous Session Compacted)\n\nThe previous conversation was compacted due to context window limitations. Below is the detailed summary of work done so far.\n\n**CRITICAL: Don't repeat already completed ACTIONS**\n\n---\n\n{trimmed}\n\n---\n\nContinue work from where the previous session left off. FOCUS only on the remaining tasks.\n"
    ))
}

/// Builds the user's task message.
pub fn task_message(task: &str) -> BaseMessage {
    BaseMessage::human(format!("TASK: {task}"))
}

/// True when the token usage exceeds 80% of the context window.
pub fn needs_compaction(total_tokens: i64, context_window: i64) -> bool {
    if context_window <= 0 {
        return false;
    }
    total_tokens as f64 > context_window as f64 * 0.8
}

/// Home and downloads paths used by the system prompt.
pub fn directories() -> (PathBuf, PathBuf) {
    let home = crate::win32::home_dir();
    (home.join("Downloads"), home)
}

/// JSON rendering helper used by tests and event payloads.
pub fn json_string(value: &str) -> Value {
    json!(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_substitution_matches_python_format() {
        let rendered = fill_template(
            "Steps: {steps}/{max_steps} at {datetime}",
            &[
                ("steps", "1".to_string()),
                ("max_steps", "25".to_string()),
                ("datetime", "Monday".to_string()),
            ],
        );
        assert_eq!(rendered, "Steps: 1/25 at Monday");
    }

    #[test]
    fn braces_are_preserved_when_escaped() {
        let rendered = fill_template("a {{b}} c {value}", &[("value", "x".to_string())]);
        assert_eq!(rendered, "a {b} c x");
    }

    #[test]
    fn unknown_placeholders_stay_literal() {
        let rendered = fill_template("hello {missing}", &[]);
        assert_eq!(rendered, "hello {missing}");
    }

    #[test]
    fn compaction_truncates_long_content() {
        let long = "x".repeat(COMPACTION_CONTENT_LIMIT + 500);
        let messages = vec![BaseMessage::human(long)];
        let rendered = format_history_for_compaction(&messages);
        assert!(rendered.contains(TRUNCATED));
        assert!(rendered.len() < COMPACTION_CONTENT_LIMIT + 200);
    }

    #[test]
    fn compaction_includes_thought_and_filtered_params() {
        let mut params = serde_json::Map::new();
        params.insert("thought".to_string(), json!("planning"));
        params.insert("loc".to_string(), json!([1, 2]));
        let messages = vec![BaseMessage::ToolMessage {
            id: "call-1".to_string(),
            name: "click_tool".to_string(),
            params,
            content: Some("ok".to_string()),
            thinking: None,
            thinking_signature: None,
            usage: None,
        }];
        let rendered = format_history_for_compaction(&messages);
        assert!(rendered.contains("THOUGHT: planning"));
        assert!(rendered.contains("TOOL CALL call-1: click_tool("));
        assert!(!rendered.contains("\"thought\""));
        assert!(rendered.contains("TOOL RESULT call-1: ok"));
    }

    #[test]
    fn compaction_response_wrapper_matches_python() {
        let wrapped = format_compaction_response("summary").expect("wrapped");
        assert!(wrapped.starts_with("\n# Context Restoration (Previous Session Compacted)"));
        assert!(wrapped.contains("CRITICAL: Don't repeat already completed ACTIONS"));
        assert!(wrapped.trim_end().ends_with("FOCUS only on the remaining tasks."));
        assert!(format_compaction_response("   ").is_none());
    }

    #[test]
    fn compaction_threshold_is_eighty_percent() {
        assert!(!needs_compaction(80, 100));
        assert!(needs_compaction(81, 100));
        assert!(!needs_compaction(1000, 0));
    }

    #[test]
    fn task_message_prefix() {
        match task_message("open notepad") {
            BaseMessage::HumanMessage { content } => assert_eq!(content, "TASK: open notepad"),
            _ => panic!("expected a human message"),
        }
    }
}
