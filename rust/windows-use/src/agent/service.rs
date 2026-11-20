//! The agent loop, mirroring `windows_use/agent/service.py` and `registry/service.py`.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::agent::context::{
    build_state_prompt, build_system_prompt, compaction_messages, format_compaction_response,
    needs_compaction, AgentMode,
};
use crate::agent::loop_guard::LoopGuard;
use crate::agent::tools::{execute_tool, ToolKind, BUILTIN_TOOLS, EXPERIMENTAL_TOOLS};
use crate::desktop::service::Desktop;
use crate::desktop::views::Browser;
use crate::events::{log_error, log_info, log_warning, AgentEvent, Event, EventType};
use crate::messages::BaseMessage;
use crate::providers::base::ChatLlm;
use crate::providers::events::LLMEventType;
use crate::providers::views::TokenUsage;
use crate::tools::{validate_params, ToolResult, ToolSchema};

const NON_TOOL_PARAMS: [&str; 1] = ["thought"];

/// Outcome of one agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResult {
    pub is_done: bool,
    pub content: Option<String>,
    pub error: Option<String>,
}

impl AgentResult {
    pub fn done(content: impl Into<String>) -> Self {
        Self { is_done: true, content: Some(content.into()), error: None }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self { is_done: false, content: None, error: Some(error.into()) }
    }
}

/// Mutable run state.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub task: Option<String>,
    pub messages: Vec<BaseMessage>,
    pub error_messages: Vec<BaseMessage>,
    pub step: usize,
    pub max_steps: usize,
    pub max_consecutive_failures: usize,
}

impl AgentState {
    pub fn new(max_steps: usize, max_consecutive_failures: usize) -> Self {
        Self {
            task: None,
            messages: Vec::new(),
            error_messages: Vec::new(),
            step: 0,
            max_steps,
            max_consecutive_failures,
        }
    }

    pub fn reset(&mut self) {
        self.task = None;
        self.messages.clear();
        self.error_messages.clear();
        self.step = 0;
    }
}

/// The tool registry.
pub struct Registry {
    tools: Vec<ToolKind>,
}

impl Registry {
    pub fn new(experimental: bool) -> Self {
        let mut tools: Vec<ToolKind> = BUILTIN_TOOLS.to_vec();
        if experimental {
            tools.extend_from_slice(&EXPERIMENTAL_TOOLS);
        }
        Self { tools }
    }

    pub fn tools(&self) -> &[ToolKind] {
        &self.tools
    }

    pub fn get_tool(&self, name: &str) -> Option<ToolKind> {
        self.tools.iter().copied().find(|tool| tool.name() == name)
    }

    /// Schemas handed to the provider, using the exact parameters generated from
    /// the Python pydantic models.
    pub fn get_tools_schema(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .map(|tool| ToolSchema {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: crate::agent::tool_schemas::parameters(tool.name()),
            })
            .collect()
    }

    /// Validates then executes a tool.
    pub fn execute(
        &self,
        tool_name: &str,
        tool_params: &Map<String, Value>,
        desktop: Option<&mut Desktop>,
    ) -> ToolResult {
        let Some(tool) = self.get_tool(tool_name) else {
            return ToolResult::failure(format!("Tool '{tool_name}' not found."));
        };

        let errors = validate_params(&tool.params(), tool_params);
        if !errors.is_empty() {
            return ToolResult::failure(format!(
                "Tool '{tool_name}' validation failed:\n{}",
                errors.join("\n")
            ));
        }

        match execute_tool(tool, tool_params, desktop) {
            Ok(content) => ToolResult::success(content),
            Err(error) => ToolResult::failure(format!("Tool '{tool_name}' execution failed:\n{error}")),
        }
    }
}

/// The agent: orchestrates the desktop, the LLM and the tool registry.
pub struct Agent {
    pub mode: AgentMode,
    pub browser: Browser,
    pub use_vision: bool,
    pub use_annotation: bool,
    pub use_accessibility: bool,
    pub auto_minimize: bool,
    pub log_to_console: bool,
    pub instructions: Vec<String>,
    pub disable_loop_detection: bool,
    pub state: AgentState,
    pub registry: Registry,
    pub desktop: Desktop,
    llm: Arc<dyn ChatLlm>,
    event: Event,
    loop_guard: LoopGuard,
    token_usage: TokenUsage,
    cached_system_prompt: Option<String>,
}

impl Agent {
    /// Creates an agent. Validation matches the Python constructor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: Arc<dyn ChatLlm>,
        mode: AgentMode,
        browser: Browser,
        use_vision: bool,
        use_annotation: bool,
        use_accessibility: bool,
        auto_minimize: bool,
        max_steps: usize,
        max_consecutive_failures: usize,
        instructions: Vec<String>,
        log_to_console: bool,
        experimental: bool,
        disable_loop_detection: bool,
    ) -> anyhow::Result<Self> {
        if max_steps == 0 {
            anyhow::bail!("max_steps must be a positive integer");
        }
        if max_consecutive_failures == 0 {
            anyhow::bail!("max_consecutive_failures must be a positive integer");
        }

        // Annotation forces vision and accessibility on, matching the Python warnings.
        let use_vision = use_vision || use_annotation;
        let use_accessibility = use_accessibility || use_annotation;

        let desktop = Desktop::new(use_vision, use_annotation, use_accessibility)?;
        let mut event = Event::new();
        if log_to_console {
            event.add_subscriber(Box::new(crate::events::ConsoleEventSubscriber));
        }

        Ok(Self {
            mode,
            browser,
            use_vision,
            use_annotation,
            use_accessibility,
            auto_minimize,
            log_to_console,
            instructions,
            disable_loop_detection,
            state: AgentState::new(max_steps, max_consecutive_failures),
            registry: Registry::new(experimental),
            desktop,
            llm,
            event,
            loop_guard: LoopGuard::default(),
            token_usage: TokenUsage::default(),
            cached_system_prompt: None,
        })
    }

    pub fn add_subscriber(&mut self, subscriber: Box<dyn crate::events::EventSubscriber>) {
        self.event.add_subscriber(subscriber);
    }

    pub fn add_callback(&mut self, callback: Arc<dyn Fn(&AgentEvent) + Send + Sync>) {
        self.event.add_callback(callback);
    }

    pub fn add_file_logging(&mut self) -> anyhow::Result<()> {
        self.event
            .add_subscriber(Box::new(crate::events::FileEventSubscriber::new(None)?));
        Ok(())
    }

    fn system_prompt(&mut self) -> anyhow::Result<String> {
        if let Some(cached) = &self.cached_system_prompt {
            return Ok(cached.clone());
        }
        let prompt = build_system_prompt(
            self.mode,
            &mut self.desktop,
            self.browser,
            self.state.max_steps,
            &self.instructions,
        );
        self.cached_system_prompt = Some(prompt.clone());
        Ok(prompt)
    }

    fn state_message(&mut self, nudge: &str) -> anyhow::Result<BaseMessage> {
        let task = self.state.task.clone().unwrap_or_default();
        let step = self.state.step;
        let max_steps = self.state.max_steps;
        let use_accessibility = self.use_accessibility;
        let use_vision = self.use_vision;

        let state = self.desktop.get_state(true)?;
        let prompt = build_state_prompt(state, &task, step, max_steps, use_accessibility, nudge);

        if use_vision {
            if let Some(screenshot) = self
                .desktop
                .desktop_state
                .as_ref()
                .and_then(|state| state.screenshot.clone())
            {
                return Ok(BaseMessage::ImageMessage {
                    content: prompt,
                    images: vec![screenshot],
                    mime_type: "image/png".to_string(),
                });
            }
        }
        Ok(BaseMessage::human(prompt))
    }

    fn emit_llm_error(&self, step: usize, attempt: usize, error: &str, error_type: &str) -> Value {
        let mut data = Map::new();
        data.insert("step".to_string(), Value::from(step));
        data.insert("attempt".to_string(), Value::from(attempt + 1));
        data.insert("provider".to_string(), Value::String(self.llm.provider().to_string()));
        data.insert("model".to_string(), Value::String(self.llm.model_name().to_string()));
        data.insert("error_type".to_string(), Value::String(error_type.to_string()));
        data.insert("error".to_string(), Value::String(error.to_string()));
        self.event.emit(AgentEvent::new(EventType::Error, data.clone()));
        Value::Object(data)
    }

    async fn maybe_compact(&mut self) -> anyhow::Result<()> {
        let metadata = self.llm.get_metadata();
        if !needs_compaction(self.token_usage.total_tokens, metadata.context_window) {
            return Ok(());
        }

        let mut history = self.state.messages.clone();
        history.extend(self.state.error_messages.clone());
        let messages = compaction_messages(&history);

        let no_tools: &[ToolSchema] = &[];
        match self.llm.invoke(&messages, no_tools).await {
            Ok(event) if event.event_type == LLMEventType::Text => {
                let content = event.content.unwrap_or_default();
                if let Some(summary) = format_compaction_response(&content) {
                    let system = self.system_prompt()?;
                    self.state.messages =
                        vec![BaseMessage::system(system), BaseMessage::human(summary)];
                    self.state.error_messages.clear();
                    self.token_usage = TokenUsage::default();
                }
            }
            Ok(_) => {}
            Err(error) => {
                let mut data = Map::new();
                data.insert("step".to_string(), Value::from(self.state.step));
                data.insert("error".to_string(), Value::String(error.to_string()));
                data.insert(
                    "error_source".to_string(),
                    Value::String("compaction".to_string()),
                );
                self.event.emit(AgentEvent::new(EventType::Error, data));
            }
        }
        Ok(())
    }

    /// Runs the agent to completion.
    pub async fn invoke(&mut self, task: &str) -> AgentResult {
        self.state.reset();
        self.state.task = Some(task.to_string());
        self.token_usage = TokenUsage::default();
        self.loop_guard.reset();

        let system = match self.system_prompt() {
            Ok(prompt) => prompt,
            Err(error) => return AgentResult::failed(error.to_string()),
        };
        self.state.messages.insert(0, BaseMessage::system(system));
        self.state
            .messages
            .push(crate::agent::context::task_message(task));

        let guard = if self.auto_minimize {
            Some(self.desktop.auto_minimize())
        } else {
            None
        };

        let result = self.main_loop().await;
        drop(guard);
        result
    }

    /// Synchronous entry point that drives the async loop on a private runtime.
    pub fn invoke_blocking(&mut self, task: &str) -> AgentResult {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return AgentResult::failed(error.to_string()),
        };
        runtime.block_on(self.invoke(task))
    }

    fn record_loop_state(&mut self) {
        if self.disable_loop_detection {
            return;
        }
        let window_id = self
            .desktop
            .desktop_state
            .as_ref()
            .and_then(|state| state.active_window.as_ref())
            .map(|window| window.handle.to_string())
            .unwrap_or_else(|| "no_window".to_string());
        let content = self
            .desktop
            .desktop_state
            .as_ref()
            .and_then(|state| state.tree_state.as_ref())
            .map(|tree| tree.interactive_elements_to_string())
            .or_else(|| {
                self.desktop
                    .desktop_state
                    .as_ref()
                    .and_then(|state| state.active_window.as_ref())
                    .map(|window| window.name.clone())
            })
            .unwrap_or_else(|| "no_content".to_string());
        self.loop_guard.record_state(&window_id, &content);
    }

    async fn main_loop(&mut self) -> AgentResult {
        let mut consecutive_failures = 0usize;

        for step in 0..self.state.max_steps {
            self.state.step = step;

            if let Err(error) = self.maybe_compact().await {
                log_error(&format!("compaction failed: {error}"));
            }

            let nudge = if self.disable_loop_detection {
                None
            } else {
                self.loop_guard.check()
            };
            let nudge_text = nudge.clone().unwrap_or_default();

            if let Some(nudge) = &nudge {
                self.event
                    .emit(AgentEvent::error(step, &format!("Loop detected: {nudge}")));
            }

            let state_message = match self.state_message(&nudge_text) {
                Ok(message) => message,
                Err(error) => return AgentResult::failed(error.to_string()),
            };
            self.state.messages.push(state_message);
            self.record_loop_state();

            // Reason: ask the LLM, retrying on failure.
            let mut message: Option<BaseMessage> = None;
            let mut last_error: Option<String> = None;
            let mut llm_errors: Vec<Value> = Vec::new();

            for attempt in 0..self.state.max_consecutive_failures {
                let mut messages = self.state.messages.clone();
                messages.extend(self.state.error_messages.clone());
                let tools = self.registry.get_tools_schema();

                match self.llm.ainvoke(&messages, tools.as_slice()).await {
                    Ok(event) => {
                        self.token_usage = event.usage.clone();
                        match event.event_type {
                            LLMEventType::ToolCall => {
                                if let Some(call) = event.tool_call {
                                    message = Some(BaseMessage::ToolMessage {
                                        id: call.id,
                                        name: call.name,
                                        params: call.params,
                                        content: None,
                                        thinking: event
                                            .thinking
                                            .as_ref()
                                            .and_then(|thinking| thinking.content.clone()),
                                        thinking_signature: event
                                            .thinking
                                            .as_ref()
                                            .and_then(|thinking| thinking.signature.clone()),
                                        usage: None,
                                    });
                                    break;
                                }
                                last_error =
                                    Some("Provider returned a tool call without details.".to_string());
                            }
                            LLMEventType::Text => {
                                last_error =
                                    Some("Provider returned text instead of a tool call.".to_string());
                                let ai = BaseMessage::AIMessage {
                                    content: event.content,
                                    thinking: None,
                                    thinking_signature: None,
                                    usage: None,
                                };
                                let human = BaseMessage::human(
                                    "Response rejected, please use the `done_tool` to respond to the user.",
                                );
                                self.state.error_messages.push(ai);
                                self.state.error_messages.push(human);
                            }
                            LLMEventType::Error => {
                                let detail = event.content.unwrap_or_else(|| {
                                    "Provider returned an error without details.".to_string()
                                });
                                last_error = Some(detail.clone());
                                llm_errors.push(self.emit_llm_error(
                                    step,
                                    attempt,
                                    &detail,
                                    "provider_error",
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        let detail = error.to_string();
                        last_error = Some(detail.clone());
                        llm_errors.push(self.emit_llm_error(step, attempt, &detail, "provider_error"));

                        if attempt + 1 < self.state.max_consecutive_failures {
                            let wait = 2u64.pow(attempt as u32 + 1);
                            log_error(&format!(
                                "Failed to get response from {} for {}.\nRetrying...({}/{})",
                                self.llm.provider(),
                                self.llm.model_name(),
                                attempt + 1,
                                self.state.max_consecutive_failures
                            ));
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        } else {
                            log_error(&format!(
                                "Failed to get response from {} for {}.\nAll {} attempts exhausted.",
                                self.llm.provider(),
                                self.llm.model_name(),
                                self.state.max_consecutive_failures
                            ));
                        }
                    }
                }
            }

            let Some(mut message) = message else {
                let error = format!(
                    "Agent failed after exhausting retries: {}",
                    last_error.unwrap_or_else(|| "unknown error".to_string())
                );
                let mut data = Map::new();
                data.insert("step".to_string(), Value::from(step));
                data.insert("error".to_string(), Value::String(error.clone()));
                data.insert("llm_errors".to_string(), Value::Array(llm_errors));
                data.insert(
                    "error_source".to_string(),
                    Value::String("llm_exhausted_retries".to_string()),
                );
                self.event.emit(AgentEvent::new(EventType::Error, data));
                return AgentResult::failed(error);
            };

            // Remove the previous state message.
            self.state.messages.pop();

            let (tool_name, tool_params) = match &message {
                BaseMessage::ToolMessage { name, params, .. } => (name.clone(), params.clone()),
                _ => return AgentResult::failed("invalid tool message"),
            };

            let thought = tool_params
                .get("thought")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.event.emit(AgentEvent::thought(step, &thought));

            if tool_name != "done_tool" {
                let filtered: Map<String, Value> = tool_params
                    .iter()
                    .filter(|(key, _)| !NON_TOOL_PARAMS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                self.event
                    .emit(AgentEvent::tool_call(step, &tool_name, &filtered));
            }

            let tool_result = self
                .registry
                .execute(&tool_name, &tool_params, Some(&mut self.desktop));

            if !self.disable_loop_detection {
                self.loop_guard
                    .record_action(&tool_name, &tool_params, tool_result.is_success);
            }

            let content = if tool_result.is_success {
                tool_result.content.clone().unwrap_or_default()
            } else {
                tool_result.error.clone().unwrap_or_default()
            };

            if let BaseMessage::ToolMessage { content: slot, .. } = &mut message {
                *slot = Some(content.clone());
            }

            if tool_result.is_success {
                self.state.messages.push(message);
            } else {
                self.state.error_messages.push(message);
            }

            if tool_name != "done_tool" || !tool_result.is_success {
                self.event.emit(AgentEvent::tool_result(
                    step,
                    &tool_name,
                    tool_result.is_success,
                    &content,
                ));
            }

            if !tool_result.is_success {
                consecutive_failures += 1;
                if consecutive_failures >= self.state.max_consecutive_failures {
                    let error = format!(
                        "Agent aborted after {} consecutive tool failures. Last error: {content}",
                        self.state.max_consecutive_failures
                    );
                    self.event.emit(AgentEvent::error(step, &error));
                    return AgentResult::failed(error);
                }
            } else {
                consecutive_failures = 0;
            }

            if tool_name == "done_tool" && tool_result.is_success {
                self.event.emit(AgentEvent::done(step, &content));
                return AgentResult::done(content);
            }
        }

        let error = format!(
            "Agent reached the maximum number of steps ({}) without completing.",
            self.state.max_steps
        );
        self.event
            .emit(AgentEvent::error(self.state.max_steps, &error));
        AgentResult::failed(error)
    }
}

/// Registry helper for callers that only need schemas.
pub fn tool_schemas(experimental: bool) -> Vec<ToolSchema> {
    Registry::new(experimental).get_tools_schema()
}

/// Shared warning helper used by the CLI.
pub fn warn(message: &str) {
    log_warning(message);
}

/// Informational log helper used by the CLI.
pub fn inform(message: &str) {
    log_info(message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_builtin_tools_only_by_default() {
        let registry = Registry::new(false);
        assert_eq!(registry.tools().len(), 11);
        assert!(registry.get_tool("click_tool").is_some());
        assert!(registry.get_tool("file_tool").is_none());
    }

    #[test]
    fn registry_includes_experimental_tools_when_enabled() {
        let registry = Registry::new(true);
        assert_eq!(registry.tools().len(), 15);
        for name in ["file_tool", "memory_tool", "multi_select_tool", "multi_edit_tool"] {
            assert!(registry.get_tool(name).is_some(), "{name} missing");
        }
    }

    #[test]
    fn unknown_tool_reports_not_found() {
        let registry = Registry::new(false);
        let result = registry.execute("nope_tool", &Map::new(), None);
        assert_eq!(result.error.as_deref(), Some("Tool 'nope_tool' not found."));
    }

    #[test]
    fn validation_errors_are_prefixed() {
        let registry = Registry::new(false);
        let result = registry.execute("click_tool", &Map::new(), None);
        let error = result.error.unwrap_or_default();
        assert!(error.starts_with("Tool 'click_tool' validation failed:"));
        assert!(error.contains("thought:Field required"));
    }

    #[test]
    fn done_tool_executes_without_desktop() {
        let registry = Registry::new(false);
        let mut params = Map::new();
        params.insert("thought".to_string(), Value::String("done".to_string()));
        params.insert("answer".to_string(), Value::String("answer".to_string()));
        let result = registry.execute("done_tool", &params, None);
        assert!(result.is_success);
        assert_eq!(result.content.as_deref(), Some("answer"));
    }

    #[test]
    fn schemas_are_generated_for_every_tool() {
        let schemas = tool_schemas(true);
        assert_eq!(schemas.len(), 15);
        let click = schemas.iter().find(|schema| schema.name == "click_tool").unwrap();
        assert!(!click.parameters.to_string().contains("\"title\""));
    }
}
