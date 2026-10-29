#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The subagent tier through the real binary. The tool is offered only when the tier is on; the command refuses
//! when it is off; an unreachable model is reported as unavailable, never answered; a bad role is refused before any call.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The shipped profile, with the micro slot pointed at a closed port so no real model is ever reached.
const PROFILE: &str = include_str!("../../../configs/local-only.toml");

fn workspace(enabled: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let profile = PROFILE.replace(
        "endpoint = \"http://127.0.0.1:8002/v1\"",
        "endpoint = \"http://127.0.0.1:9/v1\"",
    );
    let config = format!("{profile}\n[subagents]\nenabled = {enabled}\n");
    std::fs::write(dir.path().join("local-agent.toml"), config).unwrap();
    std::fs::write(
        dir.path().join("notes.txt"),
        "def add(a, b):\n    return a - b\n",
    )
    .unwrap();
    dir
}

/// Runs `agent <args> --workspace <dir>` with a clean environment and the given stdin. Returns success, stdout, stderr.
fn run(workspace: &Path, args: &[&str], stdin: &str) -> (bool, String, String) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent"))
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
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The tools one `agent mcp` server offers, from a `tools/list` call.
fn offered_tools(workspace: &Path) -> Vec<Value> {
    let request = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n";
    let (_, stdout, _) = run(workspace, &["mcp"], request);
    let reply: Value = serde_json::from_str(stdout.lines().next().expect("a reply")).unwrap();
    reply["result"]["tools"].as_array().unwrap().clone()
}

#[test]
fn the_subagent_tool_is_offered_only_when_the_tier_is_on() {
    let off = workspace(false);
    assert!(
        offered_tools(off.path())
            .iter()
            .all(|t| t["name"] != "subagent")
    );

    let on = workspace(true);
    let tools = offered_tools(on.path());
    let tool = tools
        .iter()
        .find(|t| t["name"] == "subagent")
        .expect("the subagent tool is offered when the tier is on");
    assert_eq!(
        tool["inputSchema"]["properties"]["role"]["enum"],
        json!(["explore", "summarise"])
    );
}

#[test]
fn the_command_refuses_when_the_tier_is_off() {
    let dir = workspace(false);
    let (ok, stdout, stderr) = run(
        dir.path(),
        &[
            "subagent",
            "--role",
            "explore",
            "--task",
            "what does add return?",
            "--path",
            "notes.txt",
        ],
        "",
    );
    assert!(!ok, "{stdout}");
    assert!(stderr.contains("the subagent tier is off"), "{stderr}");
}

#[test]
fn an_unreachable_model_is_reported_as_unavailable_and_nothing_is_answered() {
    let dir = workspace(true);
    let (ok, stdout, stderr) = run(
        dir.path(),
        &[
            "subagent",
            "--role",
            "explore",
            "--task",
            "what does add return?",
            "--path",
            "notes.txt",
        ],
        "",
    );
    assert!(!ok, "{stdout}");
    assert!(stderr.contains("did not answer"), "{stderr}");
    assert!(!stdout.contains("[subagent explore answer"), "{stdout}");
}

#[test]
fn a_bad_role_is_refused_by_the_tool_before_any_model_call() {
    let dir = workspace(true);
    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "subagent", "arguments": {"role": "edit", "task": "x"}}
    });
    let (_, stdout, _) = run(dir.path(), &["mcp"], &format!("{call}\n"));
    let reply: Value = serde_json::from_str(stdout.lines().next().expect("a reply")).unwrap();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert!(reply.to_string().contains("role must be"), "{reply}");
}
