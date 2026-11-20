//! Windows-Use: an AI agent that operates Windows at the GUI layer.
//!
//! This crate is a full Rust port of the Python `windows-use` project. It talks to
//! the Windows UI Automation API directly (through the `windows` crate), drives
//! mouse and keyboard input, manages virtual desktops, and speaks to any of the
//! supported LLM providers over their HTTP APIs.

pub mod agent;
pub mod cli;
pub mod desktop;
pub mod events;
pub mod markdown;
pub mod messages;
pub mod providers;
pub mod screenshot;
pub mod speech;
pub mod telemetry;
pub mod tools;
pub mod tree;
pub mod uia;
pub mod vdm;
pub mod win32;

pub use agent::service::{Agent, AgentResult, Registry};
pub use desktop::views::Browser;
pub use events::{AgentEvent, EventType};

pub const VERSION: &str = "0.8.1";

pub fn version() -> &'static str {
    VERSION
}
