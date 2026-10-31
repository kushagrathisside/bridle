#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The adversarial regression suite: hostile tool calls through the real `agent hook tool-before`.
//!
//! The cases are in `evals/security/cases.json`. Each names the verdict the policy must give: `block` (denied or
//! asked, so OpenCode would not run it), or `allow`. A case the policy allows on purpose is written as `allow` with a
//! note that points at the known limit in `docs/concepts/security.md`. A new hostile shape goes into that file, not
//! into a test, so every shape stays in one reviewable place.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The shipped profile every case runs under.
const PROFILE: &str = include_str!("../../../configs/local-only.toml");
/// The cases the suite runs.
const CASES: &str = include_str!("../../../evals/security/cases.json");

/// A repository with a secret file, so a read of it would be a real leak if the policy allowed it.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("local-agent.toml"), PROFILE).expect("write config");
    std::fs::write(dir.path().join(".env"), "API_KEY=secret\n").expect("write .env");
    std::fs::write(dir.path().join("notes.txt"), "plain notes\n").expect("write notes");
    dir
}

/// Runs `agent hook tool-before` for one call and returns the answer.
fn hook(workspace: &Path, call: &Value) -> Value {
    let payload = json!({
        "sessionID": "ses_adversarial",
        "callID": call["id"],
        "tool": call["tool"],
        "args": call["args"],
    })
    .to_string();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent"))
        .args(["hook", "tool-before", "--workspace"])
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
        .expect("spawn agent");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait for agent");
    serde_json::from_slice(&out.stdout).expect("the hook answers with JSON")
}

#[test]
fn every_hostile_case_gets_its_expected_verdict() {
    let cases: Vec<Value> = serde_json::from_str(CASES).expect("the cases file is JSON");
    assert!(
        cases.len() >= 60,
        "the suite is too small to mean anything: {}",
        cases.len()
    );
    let dir = workspace();
    let mut failures = Vec::new();
    for case in &cases {
        let answer = hook(dir.path(), case);
        let blocked = answer["action"] != "allow";
        let expect = case["expect"].as_str().expect("expect");
        let ok = match expect {
            "block" => blocked,
            "allow" => !blocked,
            other => panic!("unknown expectation `{other}` in {}", case["id"]),
        };
        if !ok {
            failures.push(format!(
                "{} expected {expect}: {} {} -> {} ({})",
                case["id"], case["tool"], case["args"], answer["action"], answer["message"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a_plain_read_of_a_non_secret_file_is_allowed_by_the_same_suite() {
    // The suite must not pass by blocking everything: an ordinary read still goes through.
    let dir = workspace();
    let answer = hook(
        dir.path(),
        &json!({"id": "plain", "tool": "read", "args": {"filePath": "notes.txt"}}),
    );
    assert_eq!(answer["action"], "allow", "{answer}");
}
