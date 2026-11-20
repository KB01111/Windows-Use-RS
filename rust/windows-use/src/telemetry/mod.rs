//! Telemetry, mirroring `windows_use/telemetry/`.
//!
//! Captures anonymous product usage to PostHog. Telemetry is on by default and
//! can be disabled with `ANONYMIZED_TELEMETRY=false`; every failure is swallowed
//! so telemetry can never break the agent.

pub mod service;
pub mod views;

pub use service::ProductTelemetry;
pub use views::{AgentTelemetryEvent, BaseTelemetryEvent, TelemetryEvent};
