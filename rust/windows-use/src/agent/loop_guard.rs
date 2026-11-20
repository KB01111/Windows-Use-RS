//! Loop detection, mirroring `windows_use/agent/loop.py`.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{Map, Value};

const EXEMPT: [&str; 2] = ["done_tool", "wait_tool"];
const IGNORE_PARAMS: [&str; 1] = ["thought"];

const REPEAT_NUDGES: [(usize, &str); 3] = [
    (
        3,
        "You have repeated the same action {n} times — make sure it is actually making progress.",
    ),
    (
        5,
        "Same action repeated {n} times. If you are stuck, try a different approach.",
    ),
    (
        8,
        "Same action repeated {n} times. Stop and take a completely different strategy.",
    ),
];

/// Watches for repeated actions, UI stagnation, state cycles and failed retries.
pub struct LoopGuard {
    window: usize,
    hashes: VecDeque<String>,
    last_state: Option<(String, String)>,
    stagnant: usize,
    visited_states: BTreeMap<String, usize>,
    cycle_warning: String,
    last_action_key: Option<String>,
    last_action_failed: bool,
    failed_retry_warning: String,
}

impl Default for LoopGuard {
    fn default() -> Self {
        Self::new(15)
    }
}

impl LoopGuard {
    pub fn new(window: usize) -> Self {
        Self {
            window,
            hashes: VecDeque::with_capacity(window),
            last_state: None,
            stagnant: 0,
            visited_states: BTreeMap::new(),
            cycle_warning: String::new(),
            last_action_key: None,
            last_action_failed: false,
            failed_retry_warning: String::new(),
        }
    }

    /// Records one executed tool call.
    pub fn record_action(&mut self, tool: &str, params: &Map<String, Value>, is_success: bool) {
        if EXEMPT.contains(&tool) {
            return;
        }

        let mut normalised = Map::new();
        for (key, value) in params {
            if IGNORE_PARAMS.contains(&key.as_str()) {
                continue;
            }
            let entry = match value {
                Value::String(text) => Value::String(text.trim().to_lowercase()),
                other => other.clone(),
            };
            normalised.insert(key.clone(), entry);
        }

        let mut wrapper = Map::new();
        wrapper.insert(tool.to_string(), Value::Object(normalised));
        let key = hash_short(&serde_json::to_string(&Value::Object(wrapper)).unwrap_or_default(), 12);

        if self.hashes.len() == self.window {
            self.hashes.pop_front();
        }
        self.hashes.push_back(key.clone());

        if !is_success && self.last_action_key.as_deref() == Some(key.as_str()) && self.last_action_failed
        {
            self.failed_retry_warning = format!(
                "You are retrying '{tool}' with the same parameters after it already failed. \
The same action will produce the same failure — change your approach, \
try different parameters, or use a different tool."
            );
        } else {
            self.failed_retry_warning.clear();
        }

        self.last_action_key = Some(key);
        self.last_action_failed = !is_success;
    }

    /// Records the current desktop fingerprint.
    pub fn record_state(&mut self, window_id: &str, content_text: &str) {
        let digest = hash_short(content_text, 16);
        let state = (window_id.to_string(), digest.clone());

        if self.last_state.as_ref() == Some(&state) {
            self.stagnant += 1;
        } else {
            self.stagnant = 0;
        }
        self.last_state = Some(state);

        let fingerprint = format!("{window_id}|{digest}");
        let count = self.visited_states.get(&fingerprint).copied().unwrap_or(0) + 1;
        self.visited_states.insert(fingerprint, count);

        if count == 2 {
            self.cycle_warning = "You have returned to a UI state you already visited. \
You may be cycling between states. Try a completely different approach \
instead of repeating the same steps."
                .to_string();
        } else if count >= 3 {
            self.cycle_warning = format!(
                "You have returned to this state {count} times. \
You are stuck in a loop — stop and reconsider your strategy entirely. \
Try an alternative method or approach."
            );
        } else {
            self.cycle_warning.clear();
        }
    }

    /// Warning text for the next prompt, if any.
    pub fn check(&self) -> Option<String> {
        let mut warnings: Vec<String> = Vec::new();

        if !self.hashes.is_empty() {
            let mut counts: BTreeMap<&String, usize> = BTreeMap::new();
            for hash in &self.hashes {
                *counts.entry(hash).or_insert(0) += 1;
            }
            let top = counts.values().copied().max().unwrap_or(0);
            for (threshold, template) in REPEAT_NUDGES.iter().rev() {
                if top >= *threshold {
                    warnings.push(template.replace("{n}", &top.to_string()));
                    break;
                }
            }
        }

        if self.stagnant >= 4 {
            warnings.push(format!(
                "The UI has not changed for {} steps. \
Your actions may not be having any effect. \
If waiting for something external (OTP, email, slow load) \
use wait_tool first, then re-check.",
                self.stagnant
            ));
        }

        if !self.cycle_warning.is_empty() {
            warnings.push(self.cycle_warning.clone());
        }
        if !self.failed_retry_warning.is_empty() {
            warnings.push(self.failed_retry_warning.clone());
        }

        if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("\n\n"))
        }
    }

    pub fn reset(&mut self) {
        self.hashes.clear();
        self.last_state = None;
        self.stagnant = 0;
        self.visited_states.clear();
        self.cycle_warning.clear();
        self.last_action_key = None;
        self.last_action_failed = false;
        self.failed_retry_warning.clear();
    }
}

/// SHA-256 of the input as lowercase hex, truncated to `length` characters.
fn hash_short(text: &str, length: usize) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    hex.chars().take(length).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        map
    }

    #[test]
    fn repeated_actions_trigger_nudges() {
        let mut guard = LoopGuard::new(15);
        let args = params(&[("loc", json!([10, 20]))]);
        for _ in 0..3 {
            guard.record_action("click_tool", &args, true);
        }
        let warning = guard.check().expect("warning");
        assert!(warning.contains("repeated the same action 3 times"));
    }

    #[test]
    fn thought_is_ignored_in_action_identity() {
        let mut guard = LoopGuard::new(15);
        let first = params(&[("loc", json!([10, 20])), ("thought", json!("a"))]);
        let second = params(&[("loc", json!([10, 20])), ("thought", json!("b"))]);
        guard.record_action("click_tool", &first, true);
        guard.record_action("click_tool", &second, true);
        guard.record_action("click_tool", &first, true);
        assert!(guard.check().is_some());
    }

    #[test]
    fn exempt_tools_are_not_tracked() {
        let mut guard = LoopGuard::new(15);
        let args = params(&[]);
        for _ in 0..10 {
            guard.record_action("done_tool", &args, true);
            guard.record_action("wait_tool", &args, true);
        }
        assert!(guard.check().is_none());
    }

    #[test]
    fn stagnation_is_detected() {
        let mut guard = LoopGuard::new(15);
        // The first record establishes the baseline; each identical state after it
        // increments the stagnation counter (so 5 records reach 4 stagnant steps).
        for _ in 0..5 {
            guard.record_state("123", "same content");
        }
        let warning = guard.check().expect("warning");
        assert!(warning.contains("The UI has not changed for 4 steps"));
    }

    #[test]
    fn cycle_is_detected() {
        let mut guard = LoopGuard::new(15);
        guard.record_state("1", "a");
        guard.record_state("2", "b");
        guard.record_state("1", "a");
        let warning = guard.check().expect("warning");
        assert!(warning.contains("returned to a UI state you already visited"));
    }

    #[test]
    fn failed_retry_is_detected() {
        let mut guard = LoopGuard::new(15);
        let args = params(&[("command", json!("bad"))]);
        guard.record_action("shell_tool", &args, false);
        guard.record_action("shell_tool", &args, false);
        let warning = guard.check().expect("warning");
        assert!(warning.contains("retrying 'shell_tool' with the same parameters"));
    }

    #[test]
    fn reset_clears_state() {
        let mut guard = LoopGuard::new(15);
        let args = params(&[("loc", json!([1, 1]))]);
        for _ in 0..5 {
            guard.record_action("click_tool", &args, true);
        }
        guard.reset();
        assert!(guard.check().is_none());
    }
}
