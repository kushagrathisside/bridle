//! `run_tests`, `run_linter` and `run_typechecker`: the declared verification steps of one kind
//! (§21, R-21.2, R-21.5).
//!
//! Steps come only from config and are classified before they run (see `run_step`). `run_tests`
//! may append targets to steps that declare `targets = true`. A target is a test file, a node id
//! such as `tests/test_calc.py::test_add`, or a test name such as `calc::tests::add`. It is
//! checked before use: a restricted character set (no shell syntax, quotes, spaces or
//! expansions), no leading `-` (no options), no `..`, no absolute or drive path, and the read
//! policy for the part before any `::`.

use std::path::Path;

use harness_core::config::{StepKind, VerificationStep, is_absolute_any_platform};
use serde_json::{Value, json};

use super::{
    McpTool, StepResult, ToolContext, ToolOutput, check_read_path, optional_string_list,
    report_steps, run_step, step_name,
};

/// The most targets one `run_tests` call may pass.
const MAX_TARGETS: usize = 20;
/// The longest target accepted.
const MAX_TARGET_LEN: usize = 300;

/// Runs the declared steps of one kind.
#[derive(Debug, Clone, Copy)]
pub struct RunKindTool {
    kind: StepKind,
    name: &'static str,
    description: &'static str,
    /// The report heading, such as `Tests`.
    label: &'static str,
}

/// `run_tests`: the `test` steps, optionally narrowed to targets.
pub const RUN_TESTS: RunKindTool = RunKindTool {
    kind: StepKind::Test,
    name: "run_tests",
    description: "Run the configured test steps, optionally only for given test files, node ids or test names.",
    label: "Tests",
};

/// `run_linter`: the `lint` steps.
pub const RUN_LINTER: RunKindTool = RunKindTool {
    kind: StepKind::Lint,
    name: "run_linter",
    description: "Run the configured lint steps and report the findings.",
    label: "Lint",
};

/// `run_typechecker`: the `typecheck` steps.
pub const RUN_TYPECHECKER: RunKindTool = RunKindTool {
    kind: StepKind::Typecheck,
    name: "run_typechecker",
    description: "Run the configured type-check steps and report the errors.",
    label: "Type check",
};

impl McpTool for RunKindTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn input_schema(&self) -> Value {
        let mut properties = json!({
            "steps": {"type": "array", "items": {"type": "string"}, "description": "Names of the steps to run; default all of this kind."},
        });
        if self.kind == StepKind::Test {
            properties["targets"] = json!({
                "type": "array",
                "items": {"type": "string"},
                "maxItems": MAX_TARGETS,
                "description": "Test files, node ids (tests/test_x.py::test_y) or test names, passed to steps that accept targets.",
            });
        }
        json!({"type": "object", "properties": properties, "additionalProperties": false})
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        if !ctx.verification.enabled {
            return Err("verification is disabled ([verification] enabled = false)".to_owned());
        }
        let requested = optional_string_list(args, "steps")?.unwrap_or_default();
        let targets = if self.kind == StepKind::Test {
            optional_string_list(args, "targets")?.unwrap_or_default()
        } else {
            Vec::new()
        };
        if targets.len() > MAX_TARGETS {
            return Err(format!("at most {MAX_TARGETS} targets per call"));
        }
        for target in &targets {
            check_target(ctx, target)?;
        }

        // Names are given over the whole list, so `step-N` matches run_verification.
        let of_kind: Vec<(String, &VerificationStep)> = ctx
            .verification
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.kind == self.kind)
            .map(|(index, step)| (step_name(index, step), step))
            .collect();
        if of_kind.is_empty() {
            return Ok(ToolOutput {
                text: format!(
                    "No {kind} steps are configured. Add one to local-agent.toml:\n\n\
                     [[verification.steps]]\nname = \"...\"\ncommand = \"...\"\nkind = \"{kind}\"",
                    kind = self.kind.as_str()
                ),
                structured: Some(json!({"passed": false, "steps": []})),
                is_error: false,
            });
        }
        for wanted in &requested {
            if !of_kind.iter().any(|(name, _)| name == wanted) {
                let names: Vec<&str> = of_kind.iter().map(|(name, _)| name.as_str()).collect();
                return Err(format!(
                    "unknown {} step: {wanted}; configured: {}",
                    self.kind.as_str(),
                    names.join(", ")
                ));
            }
        }
        let selected: Vec<(String, &VerificationStep)> = of_kind
            .into_iter()
            .filter(|(name, _)| requested.is_empty() || requested.contains(name))
            .collect();
        if !targets.is_empty() && !selected.iter().any(|(_, step)| step.targets) {
            return Err(
                "none of the selected test steps accepts targets; set `targets = true` on the step \
                 in local-agent.toml"
                    .to_owned(),
            );
        }

        let results: Vec<StepResult> = selected
            .into_iter()
            .map(|(name, step)| {
                let extra: &[String] = if step.targets { &targets } else { &[] };
                run_step(ctx, name, step, extra)
            })
            .collect();
        report_steps(self.label, &results)
    }
}

/// Checks one test target before it is appended to a command (see the module docs).
pub fn check_target(ctx: &ToolContext, target: &str) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("target `{target}` refused: {why}"));
    if target.is_empty() || target.len() > MAX_TARGET_LEN {
        return refuse("empty or too long");
    }
    if target.starts_with('-') {
        return refuse("options cannot be passed as targets");
    }
    if !target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./\\:-=,@+".contains(c))
    {
        return refuse("only letters, digits and _ . / \\ : - = , @ + are allowed");
    }
    // `::` separates node ids and Rust paths; any other colon is a drive (`C:x`).
    if target.replace("::", "").contains(':') {
        return refuse("drive paths are not allowed");
    }
    let path_part = target.split("::").next().unwrap_or(target);
    if path_part.split(['/', '\\']).any(|part| part == "..")
        || path_part.starts_with(['/', '\\'])
        || is_absolute_any_platform(Path::new(path_part))
    {
        return refuse("targets must stay inside the workspace");
    }
    // The first part may be a path the runner opens (`.env` has no extension, so every target is
    // checked): it must stay in the workspace and must not be a secret file.
    check_read_path(ctx, path_part)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;

    fn step(name: &str, command: &str, kind: StepKind, targets: bool) -> VerificationStep {
        VerificationStep {
            name: Some(name.to_owned()),
            command: command.to_owned(),
            timeout_ms: None,
            cwd: None,
            kind,
            targets,
        }
    }

    fn names(output: &ToolOutput) -> Vec<String> {
        output.structured.as_ref().unwrap()["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn each_tool_runs_only_its_kind() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_context(dir.path());
        ctx.verification.steps = vec![
            step("unit", "echo tests", StepKind::Test, false),
            step("style", "echo lint", StepKind::Lint, false),
            step("types", "echo types", StepKind::Typecheck, false),
        ];
        assert_eq!(names(&RUN_TESTS.call(&json!({}), &ctx).unwrap()), ["unit"]);
        assert_eq!(
            names(&RUN_LINTER.call(&json!({}), &ctx).unwrap()),
            ["style"]
        );
        assert_eq!(
            names(&RUN_TYPECHECKER.call(&json!({}), &ctx).unwrap()),
            ["types"]
        );
        let none = RUN_LINTER.call(&json!({"steps": ["unit"]}), &ctx);
        assert!(none.unwrap_err().contains("unknown lint step"));
    }

    #[test]
    fn a_missing_kind_explains_how_to_declare_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_context(dir.path());
        ctx.verification.steps = vec![step("unit", "echo tests", StepKind::Test, false)];
        let out = RUN_TYPECHECKER.call(&json!({}), &ctx).unwrap();
        assert!(out.text.contains("kind = \"typecheck\""), "{}", out.text);
    }

    #[test]
    fn targets_reach_only_steps_that_accept_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_calc.py"), "").unwrap();
        let mut ctx = test_context(dir.path());
        ctx.verification.steps = vec![
            step("focused", "echo run", StepKind::Test, true),
            step("whole", "echo all", StepKind::Test, false),
        ];
        let out = RUN_TESTS
            .call(
                &json!({"targets": ["tests/test_calc.py::test_add", "calc::tests::add"]}),
                &ctx,
            )
            .unwrap();
        let steps = out.structured.unwrap()["steps"].clone();
        assert_eq!(
            steps[0]["command"],
            json!("echo run tests/test_calc.py::test_add calc::tests::add")
        );
        assert_eq!(steps[1]["command"], json!("echo all"));
        assert_eq!(steps[0]["status"], json!("passed"));

        ctx.verification.steps = vec![step("whole", "echo all", StepKind::Test, false)];
        let refused = RUN_TESTS.call(&json!({"targets": ["tests"]}), &ctx);
        assert!(refused.unwrap_err().contains("targets = true"));
    }

    #[test]
    fn unsafe_targets_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        for target in [
            "",
            "-p evil",
            "--rootdir=/",
            "../outside.py",
            "a/../../x",
            "/etc/passwd",
            "C:\\x",
            "C:x",
            "a;b",
            "a&&b",
            "a|b",
            "$HOME",
            "%PATH%",
            "x y",
            "'q'",
            "test_x[1]",
            ".env",
            "conf/.env",
        ] {
            assert!(check_target(&ctx, target).is_err(), "{target}");
        }
        for target in [
            "tests",
            "tests/test_calc.py",
            "tests\\test_calc.py::Cls::test_x",
            "calc::tests::add",
            "TestSuite.Case",
        ] {
            assert!(check_target(&ctx, target).is_ok(), "{target}");
        }
    }
}
