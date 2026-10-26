//! Records verification results for the orchestrator (M3 step 6).
//!
//! The after-hook of the host does not see MCP output, so the server writes each verification result to the
//! workspace state itself, and the host's after-hook for the same call claims it. The fingerprints are
//! computed here, on the raw output and before redaction (see the M3 plan). Only the hashes are stored.

use harness_core::fingerprint::{Context, Fields, args_hash, fingerprint_fields};
use harness_core::journal::Outcome;
use serde_json::Value;

use super::tools::ToolContext;
use crate::state::StateDir;

/// The verification tools whose results are recorded.
pub const VERIFICATION_TOOLS: [&str; 4] = [
    "run_verification",
    "run_tests",
    "run_linter",
    "run_typechecker",
];

/// Whether a tool is one of the verification tools.
pub fn is_verification_tool(name: &str) -> bool {
    VERIFICATION_TOOLS.contains(&name)
}

/// Records one verification result when the workflow is enabled. A failure to record is reported on
/// stderr: the server keeps serving, and the claim on the host side then finds nothing.
pub fn record(ctx: &ToolContext, tool: &str, args: &Value, structured: Option<&Value>) {
    if !ctx.config.workflow.enabled || !is_verification_tool(tool) {
        return;
    }
    let (outcome, fingerprints) = read_result(structured);
    let stored = StateDir::open(&ctx.workspace)
        .and_then(|state| state.append_mcp_result(tool, &args_hash(args), outcome, fingerprints));
    if let Err(error) = stored {
        eprintln!("harness mcp: cannot record the {tool} result: {error}");
    }
}

/// The outcome and the fingerprints of a verification result. Without a structured result the outcome
/// is unknown, and an unknown outcome is never a pass.
pub fn read_result(structured: Option<&Value>) -> (Outcome, Vec<String>) {
    let Some(value) = structured else {
        return (Outcome::Unknown, Vec::new());
    };
    let outcome = match value.get("passed").and_then(Value::as_bool) {
        Some(true) => Outcome::Passed,
        Some(false) => Outcome::Failed,
        None => Outcome::Unknown,
    };
    let mut fingerprints = Vec::new();
    for step in value
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let step_name = step.get("name").and_then(Value::as_str).unwrap_or("");
        let context = Context {
            command: Some(step_name).filter(|name| !name.is_empty()),
            exit_code: step
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
        };
        let parsed = step.get("failures").and_then(Value::as_array);
        let mut step_fingerprints: Vec<String> = parsed
            .into_iter()
            .flatten()
            .map(|failure| {
                let text = |key: &str| failure.get(key).and_then(Value::as_str);
                fingerprint_fields(
                    &Fields {
                        kind: text("kind").unwrap_or("unknown"),
                        tool: text("tool").unwrap_or(""),
                        name: text("name"),
                        file: text("file"),
                        message: text("message").unwrap_or(""),
                    },
                    context,
                )
            })
            .collect();
        // A failed step whose output the parsers did not recognise still counts as a failure, so
        // repeated silent failures are detected. Its last line of output is the message.
        let failed = step.get("status").and_then(Value::as_str) != Some("passed");
        if failed && step_fingerprints.is_empty() {
            let last_line = step
                .get("output_tail")
                .and_then(Value::as_str)
                .and_then(|tail| tail.lines().rev().find(|line| !line.trim().is_empty()))
                .unwrap_or("");
            step_fingerprints.push(fingerprint_fields(
                &Fields {
                    kind: "unparsed",
                    tool: "step",
                    name: Some(step_name),
                    file: None,
                    message: last_line,
                },
                context,
            ));
        }
        fingerprints.extend(step_fingerprints);
    }
    (outcome, fingerprints)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_passing_result_is_a_pass_with_no_fingerprints() {
        let structured = json!({"passed": true, "steps": [{"name": "unit", "status": "passed", "exit_code": 0, "failures": [], "output_tail": ""}]});
        assert_eq!(
            read_result(Some(&structured)),
            (Outcome::Passed, Vec::new())
        );
    }

    #[test]
    fn a_failing_step_gives_one_fingerprint_per_parsed_failure() {
        let structured = json!({"passed": false, "steps": [{
            "name": "unit", "status": "failed", "exit_code": 1, "output_tail": "FAILED",
            "failures": [{"tool": "pytest", "kind": "test_failure", "name": "test_add", "file": "tests/test_calc.py", "line": 5, "message": "assert -1 == 3"}]
        }]});
        let (outcome, fingerprints) = read_result(Some(&structured));
        assert_eq!(outcome, Outcome::Failed);
        assert_eq!(fingerprints.len(), 1);
        assert!(fingerprints[0].starts_with("fp_v1:"));
    }

    #[test]
    fn a_failed_step_without_parsed_failures_is_still_a_failure() {
        let structured = json!({"passed": false, "steps": [{
            "name": "build", "status": "failed", "exit_code": 2, "failures": [],
            "output_tail": "earlier line\nlinker exploded"
        }]});
        let (outcome, fingerprints) = read_result(Some(&structured));
        assert_eq!(outcome, Outcome::Failed);
        assert_eq!(fingerprints.len(), 1);
    }

    #[test]
    fn a_missing_structured_result_is_unknown_never_a_pass() {
        assert_eq!(read_result(None), (Outcome::Unknown, Vec::new()));
        assert_eq!(read_result(Some(&json!({"steps": []}))).0, Outcome::Unknown);
    }
}
