//! `run_command` and `inspect_config` (M2 step 4).
//!
//! `run_command` runs one program with arguments and never a shell. Every word must use a small,
//! unambiguous character set, so the argument list and the command line the policy classifies
//! are the same thing. The policy then checks the whole command as the shell would read it
//! (secret files, workspace paths, command class) and only an Allow runs. Programs named by a
//! path are refused: use a declared verification step for project scripts.
//!
//! `inspect_config` returns the effective configuration (redacted) and the validation findings.

use std::process::{Command, ExitStatus};
use std::time::Duration;

use harness_core::config::{ValidationEnv, Verdict, validate_with};
use harness_core::security::tool_policy::{ToolKind, ToolRequest};
use serde_json::{Value, json};

use super::{
    Keep, McpTool, OUTPUT_TAIL_BYTES, STEP_SHELL, ToolContext, ToolOutput, check_read_path, lossy,
    optional_string_list, redacted_output, run_child, tail_text, u64_arg,
};

/// The most arguments one `run_command` call may pass.
const MAX_ARGS: usize = 32;
/// The longest single word accepted.
const MAX_WORD_LEN: usize = 300;
/// The longest timeout a caller may ask for (ten minutes).
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Characters a word may use. No quotes, spaces, `$`, `%`, `*`, `?`, `;`, `&`, `|`, `<`, `>`,
/// `` ` `` or brackets, so no shell can read the word any other way than written.
pub(super) fn plain_word(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= MAX_WORD_LEN
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./-=:,@+".contains(c))
}

/// `run_command`: one program, its arguments, a working directory, no shell.
#[derive(Debug)]
pub struct RunCommandTool;

impl McpTool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }
    fn description(&self) -> &'static str {
        "Run one program on PATH with plain arguments (no shell syntax). The policy checks it first; \
         project scripts belong in a verification step."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "program": {"type": "string", "description": "Program name on PATH, e.g. git, python, cargo."},
                "args": {"type": "array", "items": {"type": "string"}, "maxItems": MAX_ARGS,
                         "description": "Arguments; letters, digits and _ . / - = : , @ + only."},
                "cwd": {"type": "string", "description": "Working directory inside the workspace (default: workspace root)."},
                "timeout_ms": {"type": "integer", "minimum": 1000, "maximum": MAX_TIMEOUT_MS},
            },
            "required": ["program"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let program = args
            .get("program")
            .and_then(Value::as_str)
            .ok_or("`program` must be a string")?;
        if !plain_word(program) || program.contains(['/', '\\', ':']) {
            return Err(
                "`program` must be a bare program name found on PATH; put project scripts in a \
                 verification step"
                    .to_owned(),
            );
        }
        let words = optional_string_list(args, "args")?.unwrap_or_default();
        if words.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments per call"));
        }
        if let Some(bad) = words.iter().find(|word| !plain_word(word)) {
            return Err(format!(
                "argument `{bad}` uses characters run_command does not pass; use a verification step"
            ));
        }
        let cwd = match args.get("cwd") {
            None | Some(Value::Null) => None,
            Some(Value::String(dir)) if !dir.is_empty() => {
                check_read_path(ctx, dir)?;
                Some(dir.clone())
            }
            Some(_) => return Err("`cwd` must be a non-empty string".to_owned()),
        };
        let timeout_ms = u64_arg(args, "timeout_ms", ctx.verification.default_timeout_ms)?
            .clamp(1_000, MAX_TIMEOUT_MS);

        execute_plain(ctx, program, &words, cwd, timeout_ms)
    }
}

/// Runs one program with plain words under the full policy (see the module docs). Shared by
/// `run_command` and the configured adapters, so both get the same checks.
pub(super) fn execute_plain(
    ctx: &ToolContext,
    program: &str,
    words: &[String],
    cwd: Option<String>,
    timeout_ms: u64,
) -> Result<ToolOutput, String> {
    let mut line = program.to_owned();
    for word in words {
        line.push(' ');
        line.push_str(word);
    }

    // The full policy: secret and path checks on every word, then the command class.
    let mut request = ToolRequest::new(ToolKind::Shell);
    request.command = Some(line.clone());
    request.workdir = cwd.clone();
    let decision = ctx.policy.evaluate(&request);
    let assessment = ctx.policy.assess_in(&line, STEP_SHELL);
    let allowed = decision.verdict == Verdict::Allow
        && ctx.policy.class_verdict(assessment.class) == Verdict::Allow;
    if !allowed {
        let reasons = if decision.reasons.is_empty() {
            format!("command class {}", assessment.class.as_str())
        } else {
            decision.reasons.join("; ")
        };
        let text = ctx
            .redactor
            .redact(&format!("blocked by policy ({}): {reasons}", decision.rule))
            .into_owned();
        return Ok(ToolOutput {
            text: text.clone(),
            structured: Some(json!({
                "status": "blocked",
                "command": ctx.redactor.redact(&line).into_owned(),
                "rule": decision.rule,
                "reason": text,
            })),
            is_error: false,
        });
    }

    let dir = match &cwd {
        Some(relative) => ctx.workspace.join(relative),
        None => ctx.workspace.clone(),
    };
    let mut command = Command::new(program);
    command.args(words).current_dir(&dir);
    let started = std::time::Instant::now();
    let run = run_child(
        command,
        Duration::from_millis(timeout_ms),
        Keep::Tail(64 * 1024),
    );
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let run = match run {
        Ok(run) => run,
        Err(error) => {
            return Ok(redacted_output(
                ctx,
                &format!("failed to start `{program}`: {error}"),
                Some(json!({"status": "failed", "command": line})),
            ));
        }
    };

    let (status, exit_code) = if run.timed_out {
        ("timeout", None)
    } else {
        let success = run.exit.as_ref().is_some_and(ExitStatus::success);
        (
            if success { "passed" } else { "failed" },
            run.exit.as_ref().and_then(ExitStatus::code),
        )
    };
    let stdout = ctx.redactor.redact(&lossy(&run.stdout)).into_owned();
    let stderr = ctx.redactor.redact(&lossy(&run.stderr)).into_owned();
    let output_tail = tail_text(&stdout, OUTPUT_TAIL_BYTES);
    let stderr_tail = tail_text(&stderr, OUTPUT_TAIL_BYTES);

    let mut text = format!("`{line}`: {status}");
    if let Some(code) = exit_code {
        text.push_str(&format!(", exit {code}"));
    }
    text.push_str(&format!(" ({duration_ms} ms)"));
    for part in [&output_tail, &stderr_tail] {
        for out_line in part.trim_end().lines() {
            text.push_str(&format!("\n    {out_line}"));
        }
    }
    Ok(redacted_output(
        ctx,
        &text,
        Some(json!({
            "status": status,
            "command": line,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
            "output_tail": output_tail,
            "stderr_tail": stderr_tail,
        })),
    ))
}

/// `inspect_config`: the effective configuration, redacted, and its validation findings.
#[derive(Debug)]
pub struct InspectConfigTool;

impl McpTool for InspectConfigTool {
    fn name(&self) -> &'static str {
        "inspect_config"
    }
    fn description(&self) -> &'static str {
        "Show the effective harness configuration (secrets redacted) and its validation findings."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn call(&self, _args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let mut config = serde_json::to_value(&ctx.config).map_err(|error| error.to_string())?;
        ctx.redactor.redact_json(&mut config);

        let issues = validate_with(&ctx.config, &ValidationEnv::from_process());
        let errors = issues
            .iter()
            .filter(|issue| format!("{:?}", issue.severity) == "Error")
            .count();
        let findings: Vec<Value> = issues
            .iter()
            .map(|issue| {
                json!({
                    "severity": format!("{:?}", issue.severity).to_lowercase(),
                    "rule": issue.rule,
                    "path": issue.path,
                    "message": issue.message,
                })
            })
            .collect();

        let slots: Vec<&str> = ctx.config.models.keys().map(String::as_str).collect();
        let mut text = format!(
            "Profile {}. Model slots: {}. Findings: {} ({errors} error(s)).",
            ctx.config.profile.as_str(),
            if slots.is_empty() {
                "none".to_owned()
            } else {
                slots.join(", ")
            },
            issues.len(),
        );
        for finding in &findings {
            text.push_str(&format!(
                "\n- {} [{}] {}: {}",
                finding["severity"].as_str().unwrap_or_default(),
                finding["rule"].as_str().unwrap_or_default(),
                finding["path"].as_str().unwrap_or_default(),
                finding["message"].as_str().unwrap_or_default(),
            ));
        }
        Ok(redacted_output(
            ctx,
            &text,
            Some(json!({
                "profile": ctx.config.profile.as_str(),
                "config": config,
                "findings": findings,
                "errors": errors,
            })),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;

    #[test]
    fn plain_words_are_the_only_words_passed() {
        for ok in [
            "--version",
            "src/main.rs",
            "a=b",
            "v1.2.3",
            "user@host:path",
        ] {
            assert!(plain_word(ok), "{ok}");
        }
        for bad in [
            "", "a b", "a;b", "$HOME", "%PATH%", "*.rs", "'q'", "a|b", "x&y", "`id`",
        ] {
            assert!(!plain_word(bad), "{bad}");
        }
    }

    #[test]
    fn a_harmless_program_runs_and_reports_its_output() {
        // `git --version` has no subcommand and is blocked (fail closed); a real subcommand runs.
        let Some(dir) = super::super::test_support::init_repo() else {
            return;
        };
        let ctx = test_context(dir.path());
        let out = RunCommandTool
            .call(
                &json!({"program": "git", "args": ["status", "--short"]}),
                &ctx,
            )
            .unwrap();
        let structured = out.structured.unwrap();
        assert_eq!(structured["status"], json!("passed"), "{structured}");
        let blocked = RunCommandTool
            .call(&json!({"program": "git", "args": ["--version"]}), &ctx)
            .unwrap();
        assert_eq!(blocked.structured.unwrap()["status"], json!("blocked"));
    }

    #[test]
    fn destructive_and_secret_commands_are_blocked_before_they_run() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        for (program, args) in [
            ("git", vec!["clean", "-fdx"]),
            ("git", vec!["show", ".env"]),
            ("rm", vec!["-rf", "build"]),
        ] {
            let out = RunCommandTool
                .call(&json!({"program": program, "args": args}), &ctx)
                .unwrap();
            assert_eq!(
                out.structured.unwrap()["status"],
                json!("blocked"),
                "{program}"
            );
        }
    }

    #[test]
    fn injection_and_path_programs_are_refused_before_policy() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        assert!(
            RunCommandTool
                .call(
                    &json!({"program": "git", "args": ["--version;rm -rf x"]}),
                    &ctx
                )
                .is_err()
        );
        assert!(
            RunCommandTool
                .call(&json!({"program": "bin/tool"}), &ctx)
                .is_err()
        );
        assert!(
            RunCommandTool
                .call(&json!({"program": "git", "cwd": "../outside"}), &ctx)
                .is_err()
        );
    }

    #[test]
    fn inspect_config_redacts_and_reports_findings() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        let out = InspectConfigTool.call(&json!({}), &ctx).unwrap();
        let structured = out.structured.unwrap();
        assert!(structured["config"].is_object());
        assert!(structured["findings"].is_array());
        assert!(out.text.starts_with("Profile "), "{}", out.text);
        assert!(!structured.to_string().contains("sk-"));
    }
}
