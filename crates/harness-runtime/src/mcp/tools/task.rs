//! `task_status` (M3 step 7): the state of a task, for the model and for the person.
//!
//! The tool is offered only when `[workflow].task_status = true` (off until M3 step 10 passes). It reads the
//! task cache through the same snapshot the loop rules use, and it returns only fields that are already
//! bounded: no prompt, no command text, no tool output. The output is redacted like every other tool.

use harness_core::journal::Entry;
use serde_json::{Value, json};

use super::{McpTool, ToolContext, ToolOutput, redacted_output};
use crate::state::{INDEX_FILE, SessionIndex, StateDir};

/// How many recent calls the output lists.
const RECENT_CALLS: usize = 5;

/// The `task_status` tool.
#[derive(Debug, Default)]
pub struct TaskStatusTool;

impl McpTool for TaskStatusTool {
    fn name(&self) -> &'static str {
        "task_status"
    }

    fn description(&self) -> &'static str {
        "Phase, iteration, last action, last verification and loop counters of the current task. Read-only."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "description": "A task id such as task-0001. Default: the latest task."}
            },
            "additionalProperties": false
        })
    }

    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let state = StateDir::open(&ctx.workspace).map_err(|e| e.to_string())?;
        let id = match args.get("task_id").and_then(Value::as_str) {
            Some(id) => id.to_owned(),
            None => latest_task(&state)?,
        };
        let task = state.task(&id).map_err(|e| e.to_string())?;
        let journal = task.snapshot().map_err(|e| e.to_string())?;
        let s = &journal.state;
        let recent: Vec<Value> = journal
            .ring
            .iter()
            .rev()
            .take(RECENT_CALLS)
            .map(|call| {
                json!({
                    "tool": call.tool,
                    "verification": call.verification,
                    "passed": call.passed,
                    "failures": call.fingerprints.len(),
                })
            })
            .collect();
        let structured = json!({
            "task_id": s.task_id,
            "phase": s.phase.name(),
            "iteration": s.iteration,
            "status": s.status,
            "last_action": s.last_action,
            "last_test": s.last_test,
            "failure_count": s.failure_count,
            "consecutive_refusals": s.consecutive_refusals,
            "recent_calls": recent,
        });
        let text = format!(
            "Task {}: phase {}, iteration {}, status {:?}. Last action: {}. Failures in a row: {}. \
             Refusals in a row: {}.",
            s.task_id,
            s.phase.name(),
            s.iteration,
            s.status,
            s.last_action.as_deref().unwrap_or("none"),
            s.failure_count,
            s.consecutive_refusals,
        );
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

/// The most recently created task, from the session index.
fn latest_task(state: &StateDir) -> Result<String, String> {
    let index: SessionIndex = state
        .read_json(INDEX_FILE)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if index.next_task == 0 {
        return Err("no task exists yet; start one with `agent task start`".to_owned());
    }
    Ok(format!("task-{:04}", index.next_task))
}

/// The entries of a task, for tests and for the CLI. Kept here so both read the journal the same way.
pub fn entries(state: &StateDir, id: &str) -> Result<Vec<Entry>, String> {
    state
        .task(id)
        .and_then(|task| task.entries())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mcp::tools::test_context;

    #[test]
    fn with_no_task_the_tool_says_how_to_start_one() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        let error = TaskStatusTool.call(&json!({}), &ctx).unwrap_err();
        assert!(error.contains("agent task start"), "{error}");
    }

    #[test]
    fn the_status_of_the_latest_task_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        let state = StateDir::open(dir.path()).unwrap();
        state.bind_session("ses_status", "repo").unwrap();
        let output = TaskStatusTool.call(&json!({}), &ctx).unwrap();
        let structured = output.structured.unwrap();
        assert_eq!(structured["task_id"], json!("task-0001"));
        assert_eq!(structured["phase"], json!("START"));
        assert!(output.text.contains("task-0001"));
    }
}
