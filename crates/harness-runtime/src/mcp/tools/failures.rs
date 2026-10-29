//! `parse_test_failure`: structured failures from test, compiler, type-checker or linter output
//!. The parsing is `harness_core::failures`; failed verification steps carry the same
//! result automatically.

use harness_core::failures::{Failure, parse_failures};
use serde_json::{Value, json};

use super::{McpTool, ToolContext, ToolOutput, redacted_output};

/// The most output accepted in one call.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// One line for a failure: `name at file:line:col: message`.
pub fn describe(failure: &Failure) -> String {
    let mut text = failure.name.clone().unwrap_or_default();
    if let Some(file) = &failure.file {
        let mut at = file.clone();
        if let Some(line) = failure.line {
            at.push_str(&format!(":{line}"));
            if let Some(column) = failure.column {
                at.push_str(&format!(":{column}"));
            }
        }
        if text.is_empty() {
            text = at;
        } else {
            text.push_str(&format!(" at {at}"));
        }
    }
    if text.is_empty() {
        failure.message.clone()
    } else {
        format!("{text}: {}", failure.message)
    }
}

/// `parse_test_failure`: parses output the model already has (for example from `bash`).
#[derive(Debug)]
pub struct ParseTestFailureTool;

impl McpTool for ParseTestFailureTool {
    fn name(&self) -> &'static str {
        "parse_test_failure"
    }
    fn description(&self) -> &'static str {
        "Extract failing tests, compiler errors, type errors and lint findings (name, file, line, message) from command output."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "output": {"type": "string", "description": "Output of pytest, unittest, cargo, ctest/gtest, go test, a compiler, mypy, pyright, ruff or tsc."},
            },
            "required": ["output"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let output = args
            .get("output")
            .and_then(Value::as_str)
            .ok_or("`output` must be a string")?;
        if output.len() > MAX_OUTPUT_BYTES {
            return Err(format!(
                "output is larger than {MAX_OUTPUT_BYTES} bytes; pass the failing part"
            ));
        }
        let report = parse_failures(output);
        let mut text = if report.failures.is_empty() {
            "No failures recognised.".to_owned()
        } else {
            format!("{} failure(s):", report.failures.len() + report.omitted)
        };
        for failure in &report.failures {
            text.push_str(&format!("\n- [{}] {}", failure.tool, describe(failure)));
        }
        if report.omitted > 0 {
            text.push_str(&format!("\n- ... and {} more", report.omitted));
        }
        for summary in &report.summaries {
            text.push_str(&format!("\nsummary: {summary}"));
        }
        let structured = serde_json::to_value(&report).map_err(|error| error.to_string())?;
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;

    #[test]
    fn parses_and_redacts_output_given_by_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        let output = "FAILED tests/test_db.py::test_connect - AssertionError: DB_PASSWORD=hunter2 rejected\n\
                      ==== 1 failed in 0.1s ====";
        let out = ParseTestFailureTool
            .call(&json!({"output": output}), &ctx)
            .unwrap();
        assert!(
            out.text.contains("tests/test_db.py::test_connect"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("hunter2"), "{}", out.text);
        let structured = out.structured.unwrap();
        assert_eq!(structured["failures"][0]["tool"], json!("pytest"));
        assert!(!structured.to_string().contains("hunter2"));
        assert_eq!(structured["summaries"], json!(["1 failed in 0.1s"]));
    }

    #[test]
    fn describe_formats_names_and_locations() {
        let failure = Failure {
            tool: "cargo",
            kind: harness_core::failures::FailureKind::TestFailure,
            name: Some("calc::add".to_owned()),
            file: Some("src/calc.rs".to_owned()),
            line: Some(10),
            column: Some(9),
            message: "boom".to_owned(),
        };
        assert_eq!(describe(&failure), "calc::add at src/calc.rs:10:9: boom");
        let bare = Failure {
            name: None,
            file: None,
            line: None,
            column: None,
            ..failure
        };
        assert_eq!(describe(&bare), "boom");
    }
}
