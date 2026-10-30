//! Bounded, tolerant reading of the JSONL event logs under the state directory.
//!
//! The logs are the source of truth for the Activity, Tasks and Policy views. They are untrusted
//! input: a file may be missing, huge, truncated mid-line or hold records of another version. Only
//! the last [`MAX_READ_BYTES`] of each file are read, lines that do not parse are counted and
//! skipped, and every displayed string is redacted and stripped of control characters (a log line
//! must not be able to drive the terminal).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use harness_core::security::redact::Redactor;
use harness_runtime::event_log::{EVENTS, TOOL_EVENTS};
use serde_json::Value;

/// At most this many bytes are read from the end of each log file.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
/// Lines longer than this are skipped (the writer caps fields, so a longer line is not ours).
const MAX_LINE_BYTES: usize = 256 * 1024;
/// At most this many events are kept (the newest).
pub const MAX_EVENTS: usize = 5000;
/// Displayed strings are cut to this many characters.
const MAX_DISPLAY_CHARS: usize = 400;

/// How an event was decided, as a display category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl Decision {
    pub fn label(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// Whether the call did not go ahead (Bridle never treats `ask` as allowed).
    pub fn is_blocked(self) -> bool {
        self != Self::Allow
    }
}

/// One event, reduced to sanitized display fields.
#[derive(Debug, Clone, PartialEq)]
pub struct EventView {
    pub time: String,
    pub kind: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub tool: Option<String>,
    pub tool_class: Option<String>,
    pub decision: Option<Decision>,
    pub status: Option<String>,
    pub actor: Option<String>,
    pub model: Option<String>,
    pub duration_ms: Option<u64>,
    pub rule: Option<String>,
    pub reasons: Vec<String>,
    pub command: Option<String>,
    pub paths: Vec<String>,
    pub error: Option<String>,
    pub file: &'static str,
}

impl EventView {
    /// The task id, else the session id: the key events are grouped by.
    pub fn group_key(&self) -> Option<(&str, bool)> {
        match (&self.task_id, &self.session_id) {
            (Some(t), _) => Some((t, true)),
            (None, Some(s)) => Some((s, false)),
            _ => None,
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.decision.is_some_and(Decision::is_blocked)
    }

    /// Whether this is a failed or errored outcome (not a policy block).
    pub fn is_failure(&self) -> bool {
        matches!(self.status.as_deref(), Some("failed" | "error"))
    }

    /// One-line summary for tables.
    pub fn summary(&self) -> String {
        if let Some(reason) = self.reasons.first() {
            return reason.clone();
        }
        if let Some(err) = &self.error {
            return err.clone();
        }
        if let Some(cmd) = &self.command {
            return cmd.clone();
        }
        self.paths.first().cloned().unwrap_or_default()
    }
}

/// What was read from the logs.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LogRead {
    /// Oldest first.
    pub events: Vec<EventView>,
    /// Lines that were not valid JSON objects (including a partial final line).
    pub malformed: usize,
    /// Files whose head was skipped because they exceed [`MAX_READ_BYTES`].
    pub truncated_files: Vec<&'static str>,
    /// Log files that exist but could not be read, with the reason.
    pub unreadable: Vec<String>,
    /// Whether any log file exists at all.
    pub any_file: bool,
}

/// Reads both event logs under `state_dir`. Never fails: problems are reported in the result.
pub fn read_logs(state_dir: &Path) -> LogRead {
    let redactor = Redactor::new();
    let mut out = LogRead::default();
    for name in [TOOL_EVENTS, EVENTS] {
        read_file(&state_dir.join(name), name, &redactor, &mut out);
    }
    // RFC 3339 UTC with fixed millisecond precision sorts lexically; the sort is stable.
    out.events.sort_by(|a, b| a.time.cmp(&b.time));
    if out.events.len() > MAX_EVENTS {
        let excess = out.events.len() - MAX_EVENTS;
        out.events.drain(..excess);
    }
    out
}

fn read_file(path: &Path, name: &'static str, redactor: &Redactor, out: &mut LogRead) {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            out.any_file = true;
            out.unreadable.push(format!("{name}: {}", e.kind()));
            return;
        }
    };
    out.any_file = true;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let cut = len > MAX_READ_BYTES;
    let mut bytes = Vec::new();
    let read = if cut {
        file.seek(SeekFrom::Start(len - MAX_READ_BYTES))
            .and_then(|_| file.take(MAX_READ_BYTES).read_to_end(&mut bytes))
    } else {
        file.take(MAX_READ_BYTES).read_to_end(&mut bytes)
    };
    if let Err(e) = read {
        out.unreadable.push(format!("{name}: {}", e.kind()));
        return;
    }
    if cut {
        out.truncated_files.push(name);
    }
    let mut lines = bytes.split(|b| *b == b'\n');
    if cut {
        // The first segment starts mid-line.
        lines.next();
    }
    for line in lines {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if line.len() > MAX_LINE_BYTES {
            out.malformed += 1;
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(Value::Object(map)) => out.events.push(view(&map, name, redactor)),
            _ => out.malformed += 1,
        }
    }
}

fn view(map: &serde_json::Map<String, Value>, file: &'static str, r: &Redactor) -> EventView {
    let text = |key: &str| map.get(key).and_then(Value::as_str).map(|s| clean(s, r));
    let data = map.get("data").and_then(Value::as_object);
    let dtext = |key: &str| {
        data.and_then(|d| d.get(key))
            .and_then(Value::as_str)
            .map(|s| clean(s, r))
    };
    let dlist = |key: &str| -> Vec<String> {
        data.and_then(|d| d.get(key))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .take(20)
                    .map(|s| clean(s, r))
                    .collect()
            })
            .unwrap_or_default()
    };
    let decision = match map.get("verdict").and_then(Value::as_str) {
        Some("allow") => Some(Decision::Allow),
        Some("ask") => Some(Decision::Ask),
        Some("deny") => Some(Decision::Deny),
        _ => None,
    };
    EventView {
        time: text("timestamp").unwrap_or_default(),
        kind: text("kind").unwrap_or_else(|| "unknown".to_owned()),
        task_id: text("task_id"),
        session_id: text("session_id"),
        tool: text("tool"),
        tool_class: text("tool_class"),
        decision,
        status: text("status"),
        actor: text("actor"),
        model: text("model"),
        duration_ms: map.get("duration_ms").and_then(Value::as_u64),
        rule: dtext("rule"),
        reasons: dlist("reasons"),
        command: dtext("command"),
        paths: dlist("paths"),
        error: dtext("error"),
        file,
    }
}

/// Redacts secrets, replaces control characters and cuts to a display length.
pub fn clean(text: &str, redactor: &Redactor) -> String {
    let redacted = redactor.redact(text);
    sanitize(&redacted)
}

/// Replaces control characters (including ESC) with spaces and cuts long text.
pub fn sanitize(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_DISPLAY_CHARS + 1)
        .collect();
    if out.chars().count() > MAX_DISPLAY_CHARS {
        out = out.chars().take(MAX_DISPLAY_CHARS).collect();
        out.push('…');
    }
    out
}

/// Short display form of an RFC 3339 timestamp: `MM-DD HH:MM:SS`.
pub fn short_time(ts: &str) -> String {
    match (ts.get(5..10), ts.get(11..19)) {
        (Some(d), Some(t)) => format!("{d} {t}"),
        _ => ts.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).expect("write");
    }

    #[test]
    fn missing_logs_are_empty_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let read = read_logs(dir.path());
        assert!(read.events.is_empty() && !read.any_file && read.malformed == 0);
    }

    #[test]
    fn parses_decisions_and_skips_malformed_and_partial_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = r#"{"timestamp":"2026-10-22T10:00:00.000Z","kind":"permission","tool":"bash","tool_class":"EXECUTE_RISKY","verdict":"deny","task_id":"t1","data":{"rule":"R7","reasons":["network is denied"],"command":"curl x"}}"#;
        let body = format!("{good}\nnot json\n[1,2]\n\n{{\"timestamp\":\"2026-10-22T10:00:01.0");
        write(dir.path(), TOOL_EVENTS, &body);
        let read = read_logs(dir.path());
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.malformed, 3, "garbage, non-object and partial line");
        let e = &read.events[0];
        assert_eq!(e.decision, Some(Decision::Deny));
        assert!(e.is_blocked());
        assert_eq!(e.rule.as_deref(), Some("R7"));
        assert_eq!(e.summary(), "network is denied");
        assert_eq!(e.group_key(), Some(("t1", true)));
    }

    #[test]
    fn unknown_verdicts_and_kinds_do_not_break_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            EVENTS,
            "{\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"future_kind\",\"verdict\":\"maybe\"}\n",
        );
        let read = read_logs(dir.path());
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].kind, "future_kind");
        assert_eq!(read.events[0].decision, None);
    }

    #[test]
    fn large_files_are_read_from_the_tail_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let line = "{\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"session\",\"data\":{\"error\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"}}\n";
        let count = (MAX_READ_BYTES as usize / line.len()) + 2000;
        write(dir.path(), EVENTS, &line.repeat(count));
        let read = read_logs(dir.path());
        assert_eq!(read.truncated_files, vec![EVENTS]);
        assert_eq!(
            read.malformed, 0,
            "the cut first line is dropped, not counted"
        );
        assert!(read.events.len() <= MAX_EVENTS && !read.events.is_empty());
    }

    #[test]
    fn displayed_text_is_redacted_and_has_no_control_characters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let line = serde_json::json!({
            "timestamp": "2026-01-01T00:00:00.000Z", "kind": "tool_before",
            "tool": "bash\u{1b}[2J",
            "data": {"command": "API_KEY=sk-live-0123456789abcdefghij run"}
        });
        write(dir.path(), TOOL_EVENTS, &format!("{line}\n"));
        let e = &read_logs(dir.path()).events[0];
        assert!(!e.tool.as_deref().unwrap_or("").contains('\u{1b}'));
        assert!(
            !e.command
                .as_deref()
                .unwrap_or("")
                .contains("sk-live-0123456789abcdefghij")
        );
    }

    #[test]
    fn merged_logs_are_ordered_by_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            TOOL_EVENTS,
            "{\"timestamp\":\"2026-01-01T00:00:02.000Z\",\"kind\":\"permission\"}\n",
        );
        write(
            dir.path(),
            EVENTS,
            "{\"timestamp\":\"2026-01-01T00:00:01.000Z\",\"kind\":\"session\"}\n",
        );
        let kinds: Vec<_> = read_logs(dir.path())
            .events
            .iter()
            .map(|e| e.kind.clone())
            .collect();
        assert_eq!(kinds, ["session", "permission"]);
    }

    #[test]
    fn short_time_formats_and_falls_back() {
        assert_eq!(short_time("2026-10-22T10:11:12.345Z"), "10-22 10:11:12");
        assert_eq!(short_time("bad"), "bad");
    }
}
