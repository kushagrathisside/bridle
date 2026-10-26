#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Black-box tests of the orchestrator (M3 step 6 and 8): the real `agent` binary, a fresh repository,
//! hook payloads on stdin, and the `task`, `trace` and `explain` commands.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The shipped profile every fixture starts from.
const PROFILE: &str = include_str!("../../../configs/local-only.toml");

/// A repository whose config enables the workflow (a repository may enable it, never switch it off).
fn workspace_with_workflow() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = format!("{PROFILE}\n[workflow]\nenabled = true\n");
    std::fs::write(dir.path().join("local-agent.toml"), config).expect("write config");
    std::fs::write(dir.path().join("README.md"), "# fixture\n").expect("write README");
    dir
}

/// A repository with the shipped profile only: the workflow is off.
fn workspace_without_workflow() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("local-agent.toml"), PROFILE).expect("write config");
    dir
}

/// Runs `agent <args>` in the workspace with a clean environment. Returns success and stdout.
fn agent(workspace: &Path, args: &[&str], stdin: Option<&str>) -> (bool, String) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent"));
    command
        .args(args)
        .arg("--workspace")
        .arg(workspace)
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("LOCAL_AGENT_MODELS_DIR", workspace)
        .env("LOCAL_AGENT_RUNTIMES_DIR", workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn agent");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait for agent");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Sends one `tool-before` payload and returns the parsed answer.
fn before(workspace: &Path, session: &str, call: &str, tool: &str, args: Value) -> Value {
    let payload =
        json!({"sessionID": session, "callID": call, "tool": tool, "args": args}).to_string();
    let (_, stdout) = agent(workspace, &["hook", "tool-before"], Some(&payload));
    serde_json::from_str(stdout.trim()).expect("the hook answers with JSON")
}

fn git_status() -> Value {
    json!({"command": "git status --short"})
}

/// A write the policy allows. Read-only commands are exempt from the repeated-command rule, so the loop
/// tests repeat this instead.
fn touch_note() -> Value {
    json!({"command": "touch note.txt"})
}

#[test]
fn the_fourth_identical_command_is_refused_through_the_real_hook() {
    let dir = workspace_with_workflow();
    let session = "ses_loop";
    for (i, call) in ["c1", "c2", "c3"].iter().enumerate() {
        let answer = before(dir.path(), session, call, "bash", touch_note());
        assert_eq!(answer["action"], "allow", "call {i}: {answer}");
    }
    let refused = before(dir.path(), session, "c4", "bash", touch_note());
    assert_eq!(refused["action"], "block", "{refused}");
    assert_eq!(refused["rule"], "loop_rule", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("repeated_command"),
        "{refused}"
    );
}

#[test]
fn the_refusal_is_visible_in_trace_and_explain() {
    let dir = workspace_with_workflow();
    let session = "ses_visible";
    for call in ["c1", "c2", "c3", "c4"] {
        before(dir.path(), session, call, "bash", touch_note());
    }
    let (ok, trace) = agent(dir.path(), &["trace"], None);
    assert!(ok, "{trace}");
    assert!(trace.contains("refused rule=repeated_command"), "{trace}");
    let (ok, explain) = agent(dir.path(), &["explain"], None);
    assert!(ok, "{explain}");
    assert!(explain.contains("refused"), "{explain}");
    assert!(
        explain.contains("RepeatedCommand") || explain.contains("repeated_command"),
        "{explain}"
    );
}

#[test]
fn a_disabled_workflow_never_refuses_and_writes_no_task() {
    let dir = workspace_without_workflow();
    for call in ["c1", "c2", "c3", "c4", "c5"] {
        let answer = before(dir.path(), "ses_off", call, "bash", git_status());
        assert_eq!(answer["action"], "allow", "{answer}");
    }
    assert!(!dir.path().join(".local-agent").join("tasks").exists());
}

#[test]
fn a_started_task_is_taken_by_the_next_new_session() {
    let dir = workspace_with_workflow();
    let (ok, started) = agent(dir.path(), &["task", "start", "--prompt", "fix calc"], None);
    assert!(ok, "{started}");
    assert!(started.contains("task-0001"), "{started}");
    before(dir.path(), "ses_fresh", "c1", "bash", git_status());
    let (ok, status) = agent(dir.path(), &["task", "status", "task-0001"], None);
    assert!(ok, "{status}");
    assert!(status.contains("task-0001"), "{status}");
    let (_, trace) = agent(dir.path(), &["trace", "task-0001"], None);
    assert!(trace.contains("call_started tool=bash"), "{trace}");
}

#[test]
fn a_damaged_task_cache_degrades_the_workflow_but_keeps_the_policy_answer() {
    let dir = workspace_with_workflow();
    let session = "ses_damaged";
    before(dir.path(), session, "c1", "bash", git_status());
    std::fs::write(
        dir.path().join(".local-agent/tasks/task-0001/task.json"),
        "{not json",
    )
    .expect("damage the cache");
    let answer = before(dir.path(), session, "c2", "bash", git_status());
    assert_eq!(
        answer["action"], "allow",
        "the policy still answers: {answer}"
    );
    let log = std::fs::read_to_string(dir.path().join(".local-agent/tool-events.jsonl")).unwrap();
    assert!(log.contains("workflow_degraded"), "{log}");
}

#[test]
fn a_reset_ends_the_refusals() {
    let dir = workspace_with_workflow();
    let session = "ses_reset";
    for call in ["c1", "c2", "c3", "c4"] {
        before(dir.path(), session, call, "bash", touch_note());
    }
    let (ok, reset) = agent(dir.path(), &["task", "reset", "task-0001"], None);
    assert!(ok, "{reset}");
    let answer = before(dir.path(), session, "c5", "bash", touch_note());
    assert_eq!(answer["action"], "allow", "{answer}");
}

#[test]
fn an_invalid_session_id_is_refused_by_the_policy_path_without_writing() {
    let dir = workspace_with_workflow();
    let answer = before(dir.path(), "../escape", "c1", "bash", git_status());
    assert_eq!(answer["action"], "allow", "policy still answers: {answer}");
    assert!(!dir.path().join(".local-agent").join("tasks").exists());
}

/// Runs `git -C <dir> <args>` and asserts it succeeds. The commit identity is passed on the command line.
fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .status()
        .expect("run git")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn git_status_text(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain"])
        .output()
        .expect("run git status");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_worktree_task_runs_in_its_own_folder_and_leaves_the_primary_tree_alone() {
    let dir = workspace_with_workflow();
    // A project ignores its state and its worktrees, as `agent init` and this repository do.
    std::fs::write(
        dir.path().join(".gitignore"),
        ".local-agent/\n.worktrees/\n",
    )
    .expect("write .gitignore");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", ".gitignore", "README.md"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let before = git_status_text(dir.path());

    let (ok, started) = agent(
        dir.path(),
        &["task", "start", "--worktree", "--prompt", "fix calc"],
        None,
    );
    assert!(ok, "{started}");
    assert!(started.contains("agent-task-001"), "{started}");

    let worktree = dir.path().join(".worktrees").join("agent-task-001");
    assert!(
        worktree.join("README.md").exists(),
        "the worktree has the committed files"
    );
    assert!(
        worktree.join("local-agent.toml").exists(),
        "the generated config is copied in"
    );
    assert_eq!(
        git_status_text(dir.path()),
        before,
        "the primary working tree is unchanged"
    );

    // A session that starts in the worktree is bound to the task, and its calls are recorded there.
    before_in(&worktree, "ses_worktree", "c1", "bash", touch_note());
    let (ok, trace) = agent(dir.path(), &["trace", "task-0001"], None);
    assert!(ok, "{trace}");
    assert!(
        trace.contains("worktree branch=agent-task-001 path=.worktrees/agent-task-001"),
        "{trace}"
    );
    assert!(trace.contains("call_started tool=bash"), "{trace}");
}

/// [`before`] for a workspace other than the fixture's own.
fn before_in(workspace: &Path, session: &str, call: &str, tool: &str, args: Value) -> Value {
    before(workspace, session, call, tool, args)
}

/// A repository whose decisions come from the micro model, which is not running: the port is closed.
fn workspace_with_unreachable_model() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let profile = PROFILE
        .replace(
            "endpoint = \"http://127.0.0.1:8002/v1\"",
            "endpoint = \"http://127.0.0.1:9/v1\"",
        )
        .replace(
            "[decisions]\nprovider = \"rules\"",
            "[decisions]\nprovider = \"local_lfm\"",
        );
    let config = format!("{profile}\n[workflow]\nenabled = true\n");
    std::fs::write(dir.path().join("local-agent.toml"), config).expect("write config");
    std::fs::write(dir.path().join("README.md"), "# fixture\n").expect("write README");
    dir
}

#[test]
fn an_unreachable_model_falls_back_to_the_rules_and_the_refusal_still_happens() {
    let dir = workspace_with_unreachable_model();
    let session = "ses_fallback";
    for call in ["c1", "c2", "c3"] {
        before(dir.path(), session, call, "bash", touch_note());
    }
    let refused = before(dir.path(), session, "c4", "bash", touch_note());
    assert_eq!(refused["action"], "block", "{refused}");
    let message = refused["message"].as_str().unwrap();
    assert!(message.contains("repeated_command"), "{message}");
    assert!(
        !message.contains("model's reason"),
        "a fallback has no model reason: {message}"
    );
    let (ok, explain) = agent(dir.path(), &["explain"], None);
    assert!(ok, "{explain}");
    assert!(explain.contains("fallback from local_lfm"), "{explain}");
}
