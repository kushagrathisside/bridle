//! Append-only JSONL event log under the state directory (§28). Local only, redacted on write.
//!
//! Several short-lived hook processes may append concurrently. Each event is serialised to a
//! single line and written with one `write_all` on a file opened in append mode, which keeps lines
//! intact on both NTFS and ext4 for the line sizes we produce (string fields are capped).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use harness_core::identity;
use harness_core::security::redact::Redactor;
use harness_core::telemetry::{Event, EventKind};

/// Log file for tool and permission events (R-16.1 `tool-events.jsonl`).
pub const TOOL_EVENTS: &str = "tool-events.jsonl";
/// Log file for everything else.
pub const EVENTS: &str = "events.jsonl";

#[derive(Debug)]
pub struct EventLog {
    dir: PathBuf,
    redactor: Option<Redactor>,
    max_field_bytes: usize,
    enabled: bool,
}

impl EventLog {
    /// Event log rooted at `<workspace>/.local-agent/`.
    pub fn for_workspace(
        workspace: &Path,
        redact: bool,
        max_field_bytes: usize,
        enabled: bool,
    ) -> Self {
        Self {
            dir: workspace.join(identity::STATE_DIR),
            redactor: redact.then(Redactor::new),
            max_field_bytes,
            enabled,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file_for(kind: EventKind) -> &'static str {
        match kind {
            EventKind::ToolBefore | EventKind::ToolAfter | EventKind::Permission => TOOL_EVENTS,
            _ => EVENTS,
        }
    }

    /// Redacts, truncates and appends one event. Telemetry must never break the agent, so I/O
    /// errors are returned for the caller to ignore or report, never panicked on.
    pub fn append(&self, mut event: Event) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let mut data = serde_json::Value::Object(std::mem::take(&mut event.data));
        if let Some(r) = &self.redactor {
            r.redact_json(&mut data);
        }
        truncate_strings(&mut data, self.max_field_bytes);
        if let serde_json::Value::Object(map) = data {
            event.data = map;
        }
        let mut line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
        line.push('\n');
        fs::create_dir_all(&self.dir)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(Self::file_for(event.kind)))?;
        f.write_all(line.as_bytes())
    }
}

/// Caps every string in `value` at `max` bytes (on a char boundary), marking the cut.
pub fn truncate_strings(value: &mut serde_json::Value, max: usize) {
    match value {
        serde_json::Value::String(s) if s.len() > max => {
            let mut cut = max;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            let dropped = s.len() - cut;
            s.truncate(cut);
            s.push_str(&format!("…[+{dropped} bytes]"));
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| truncate_strings(v, max)),
        serde_json::Value::Object(map) => map.values_mut().for_each(|v| truncate_strings(v, max)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries() {
        let mut v = serde_json::json!({"a": "héllo wörld", "b": ["xxxxxxxxxx"]});
        truncate_strings(&mut v, 3);
        assert!(v["a"].as_str().is_some_and(|s| s.starts_with("hé")));
        assert!(v["b"][0].as_str().is_some_and(|s| s.starts_with("xxx…")));
    }
}
