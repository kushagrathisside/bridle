//! The sandbox against the real kernel (ADR-0016). Each test runs a command under the rules and checks what it can
//! reach. A kernel without Landlock skips these tests with a message; `sandbox::support` says why.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use harness_runtime::sandbox::{self, Settings, Support};

fn available() -> bool {
    match sandbox::support() {
        Support::Available { .. } => true,
        Support::Unavailable(reason) => {
            eprintln!("skipping: {reason}");
            false
        }
    }
}

fn settings(network: bool) -> Settings {
    Settings {
        enabled: true,
        network,
    }
}

/// Runs `sh -c script` in `workspace`, confined by `settings`, and returns whether it succeeded.
fn confined(workspace: &std::path::Path, settings: &Settings, script: &str) -> bool {
    let mut command = Command::new("sh");
    command
        .args(["-c", script])
        .current_dir(workspace)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The guard keeps the command's temporary directory until the command has finished.
    let _scratch =
        sandbox::confine(&mut command, workspace, settings).expect("the sandbox can be applied");
    command.status().expect("the command starts").success()
}

/// A path in the home directory that the sandbox does not list. The test creates and removes it.
fn home_probe(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME is set");
    PathBuf::from(home).join(format!(
        ".bridle-sandbox-probe-{name}-{}",
        std::process::id()
    ))
}

#[test]
fn a_confined_command_writes_inside_the_workspace() {
    if !available() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("inside.txt");
    let script = format!("echo ok > {}", target.display());
    assert!(confined(workspace.path(), &settings(false), &script));
    assert!(target.exists());
}

#[test]
fn a_confined_command_cannot_write_into_the_home_directory() {
    if !available() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let outside = home_probe("write");
    let script = format!("echo leaked > {}", outside.display());
    let wrote = confined(workspace.path(), &settings(false), &script);
    let existed = outside.exists();
    let _ = std::fs::remove_file(&outside);
    assert!(!wrote, "the command must be refused");
    assert!(!existed, "no file may be created in the home directory");
}

#[test]
fn a_confined_command_cannot_read_a_file_in_the_home_directory() {
    if !available() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let secret = home_probe("read");
    std::fs::write(&secret, "credential\n").unwrap();
    let script = format!("cat {} > /dev/null", secret.display());
    let read = confined(workspace.path(), &settings(false), &script);
    let _ = std::fs::remove_file(&secret);
    assert!(!read, "the command must not read the home directory");
}

#[test]
fn a_confined_command_cannot_connect_to_the_network_by_default() {
    if !available() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let workspace = tempfile::tempdir().unwrap();
    let script = format!(
        "python3 -c \"import socket; socket.create_connection(('127.0.0.1', {port}), timeout=3)\""
    );
    let connected = confined(workspace.path(), &settings(false), &script);
    assert!(
        !connected,
        "a connection must be refused without network access"
    );
}

#[test]
fn a_confined_command_connects_when_the_network_is_allowed() {
    if !available() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let workspace = tempfile::tempdir().unwrap();
    let script = format!(
        "python3 -c \"import socket; socket.create_connection(('127.0.0.1', {port}), timeout=3)\""
    );
    assert!(confined(workspace.path(), &settings(true), &script));
}

#[test]
fn a_confined_command_writes_to_its_own_temporary_directory_but_not_to_the_shared_tmp() {
    if !available() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let shared = PathBuf::from(format!(
        "/tmp/bridle-sandbox-shared-probe-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&shared);
    let script = format!(
        "echo ok > \"$TMPDIR/own.txt\" && echo leak > {}",
        shared.display()
    );
    let succeeded = confined(workspace.path(), &settings(false), &script);
    let leaked = shared.exists();
    let _ = std::fs::remove_file(&shared);
    assert!(!succeeded, "the shared /tmp must not be writable");
    assert!(!leaked, "no file may be created in the shared /tmp");
}

#[test]
fn a_command_temporary_directory_is_removed_when_the_guard_is_dropped() {
    if !available() {
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let mut command = Command::new("true");
    let scratch = sandbox::confine(&mut command, workspace.path(), &settings(false)).unwrap();
    let dir = scratch.path().expect("the sandbox is on").to_path_buf();
    assert!(dir.is_dir());
    drop(scratch);
    assert!(!dir.exists(), "the temporary directory must be removed");
}
