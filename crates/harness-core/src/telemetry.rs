//! Structured telemetry events (§28). Every agent action becomes one JSON line. Local only.

use serde::{Deserialize, Serialize};

use crate::config::Verdict;
use crate::security::ToolClass;

/// What kind of action an event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ToolBefore,
    ToolAfter,
    Permission,
    ShellEnv,
    Session,
    ModelCall,
    HealthCheck,
    Decision,
    Verification,
    Mcp,
    Doctor,
    /// The per-request view of tool results, and the task state put into the system prompt (M6).
    Context,
}

/// One telemetry record. Optional fields are omitted from the JSON when unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339 UTC timestamp with millisecond precision.
    pub timestamp: String,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Agent name (`build`, `explore`, ...) or harness component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_class: Option<ToolClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// `ok`, `failed`, `blocked`, `error`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    /// Free-form, already-redacted details.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
}

impl Event {
    /// New event of `kind` stamped with the current time.
    pub fn now(kind: EventKind) -> Self {
        Self {
            timestamp: humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string(),
            kind,
            task_id: None,
            session_id: None,
            call_id: None,
            phase: None,
            actor: None,
            model: None,
            tool: None,
            tool_class: None,
            verdict: None,
            duration_ms: None,
            status: None,
            failure_fingerprint: None,
            tokens_in: None,
            tokens_out: None,
            data: serde_json::Map::new(),
        }
    }

    /// Adds a `data` entry (builder style).
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.data.insert(key.to_owned(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_fields_are_omitted() {
        let e = Event::now(EventKind::Doctor).with("ok", true);
        let json = serde_json::to_value(&e).expect("serialise");
        let obj = json.as_object().expect("object");
        assert!(obj.contains_key("timestamp"));
        assert!(!obj.contains_key("tool"));
        assert_eq!(obj["data"]["ok"], serde_json::Value::Bool(true));
        assert_eq!(obj["kind"], "doctor");
    }
}
