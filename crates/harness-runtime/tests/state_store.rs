#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Behaviour of the M3 state store: envelopes, redaction, torn lines, locks and atomic writes.

use std::fs::{self, File};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use harness_runtime::state::{FileLock, StateDir, StateError, atomic_write};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    name: String,
    count: u32,
}

fn open() -> (tempfile::TempDir, StateDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDir::open(dir.path()).unwrap();
    (dir, state)
}

fn sample(name: &str, count: u32) -> Sample {
    Sample {
        name: name.to_owned(),
        count,
    }
}

#[test]
fn json_round_trips_inside_a_versioned_envelope() {
    let (_dir, state) = open();
    state.write_json("task.json", &sample("t1", 2)).unwrap();
    let text = fs::read_to_string(state.path("task.json")).unwrap();
    assert!(text.contains("\"schema\": 1"), "{text}");
    let back: Option<Sample> = state.read_json("task.json").unwrap();
    assert_eq!(back, Some(sample("t1", 2)));
}

#[test]
fn a_missing_file_reads_as_none() {
    let (_dir, state) = open();
    let back: Option<Sample> = state.read_json("missing.json").unwrap();
    assert_eq!(back, None);
}

#[test]
fn a_newer_schema_is_refused_and_both_versions_are_named() {
    let (_dir, state) = open();
    let path = state.path("task.json");
    fs::write(&path, r#"{"schema": 2, "data": {"name": "x", "count": 1}}"#).unwrap();
    let error = state.read_json::<Sample>("task.json").unwrap_err();
    match &error {
        StateError::NewerSchema {
            found, supported, ..
        } => {
            assert_eq!((*found, *supported), (2, 1));
        }
        other => panic!("expected NewerSchema, got {other:?}"),
    }
    let message = error.to_string();
    assert!(
        message.contains("schema 2") && message.contains("schema 1"),
        "{message}"
    );
}

#[test]
fn unknown_fields_inside_data_are_tolerated() {
    let (_dir, state) = open();
    fs::write(
        state.path("task.json"),
        r#"{"schema": 1, "data": {"name": "x", "count": 1, "added_later": true}}"#,
    )
    .unwrap();
    let back: Option<Sample> = state.read_json("task.json").unwrap();
    assert_eq!(back, Some(sample("x", 1)));
}

#[test]
fn corrupt_json_is_reported_with_its_file_name() {
    let (_dir, state) = open();
    fs::write(state.path("task.json"), "{not json").unwrap();
    let error = state.read_json::<Sample>("task.json").unwrap_err();
    match error {
        StateError::Corrupt { path, .. } => assert!(path.ends_with("task.json")),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn secrets_are_redacted_before_they_reach_the_disk() {
    let (_dir, state) = open();
    state
        .write_json("state.json", &json!({"note": "export HF_TOKEN=hunter2"}))
        .unwrap();
    let text = fs::read_to_string(state.path("state.json")).unwrap();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("[REDACTED]"), "{text}");
}

#[test]
fn appended_records_are_read_back_in_order() {
    let (_dir, state) = open();
    for count in 0..5 {
        state
            .append("decisions.jsonl", &sample("d", count))
            .unwrap();
    }
    let read = state.read_records::<Sample>("decisions.jsonl").unwrap();
    assert_eq!(read.torn_lines, 0);
    let counts: Vec<u32> = read.records.iter().map(|r| r.count).collect();
    assert_eq!(counts, vec![0, 1, 2, 3, 4]);
}

#[test]
fn a_missing_log_reads_as_empty() {
    let (_dir, state) = open();
    let read = state.read_records::<Sample>("failures.jsonl").unwrap();
    assert!(read.records.is_empty());
    assert_eq!(read.torn_lines, 0);
}

#[test]
fn a_torn_last_line_is_skipped_and_counted() {
    let (_dir, state) = open();
    let path = state.path("events.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"schema": 1, "data": {"name": "a", "count": 1}}"#,
            "\n",
            r#"{"schema": 1, "data": {"name": "b", "count": 2}}"#,
            "\n",
            r#"{"schema": 1, "data": {"name": "c""#,
        ),
    )
    .unwrap();
    let read = state.read_records::<Sample>("events.jsonl").unwrap();
    assert_eq!(read.records.len(), 2);
    assert_eq!(read.torn_lines, 1);
}

#[test]
fn a_damaged_line_in_the_middle_is_an_error_with_its_line_number() {
    let (_dir, state) = open();
    let path = state.path("events.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"schema": 1, "data": {"name": "a", "count": 1}}"#,
            "\n",
            "BAD LINE\n",
            r#"{"schema": 1, "data": {"name": "b", "count": 2}}"#,
            "\n",
        ),
    )
    .unwrap();
    let error = state.read_records::<Sample>("events.jsonl").unwrap_err();
    assert!(error.to_string().contains("line 2"), "{error}");
}

#[test]
fn concurrent_appends_are_all_kept_and_none_is_torn() {
    let (_dir, state) = open();
    let state = Arc::new(state);
    let handles: Vec<_> = (0..4)
        .map(|thread_id| {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                for count in 0..25 {
                    state
                        .append("events.jsonl", &sample(&format!("t{thread_id}"), count))
                        .unwrap();
                    // Hook calls arrive from separate processes, so they are spaced out in time.
                    // A tight loop would keep one writer holding the lock and starve the polling waiters.
                    thread::sleep(Duration::from_millis(5));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let read = state.read_records::<Sample>("events.jsonl").unwrap();
    assert_eq!(read.records.len(), 100);
    assert_eq!(read.torn_lines, 0);
}

#[test]
fn stale_temporary_files_are_removed_when_the_state_opens() {
    let (dir, state) = open();
    let old = state.path("state.json.tmp-999-0");
    let recent = state.path("state.json.tmp-999-1");
    fs::write(&old, "partial").unwrap();
    fs::write(&recent, "partial").unwrap();
    let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();

    StateDir::open(dir.path()).unwrap();

    assert!(
        !old.exists(),
        "a temporary file from a killed process should be removed"
    );
    assert!(
        recent.exists(),
        "a temporary file that may still be in use should be kept"
    );
}

#[test]
fn a_held_lock_times_out_for_another_taker() {
    let (_dir, state) = open();
    let _held = FileLock::acquire(&state.path(".lock"), Duration::from_secs(1)).unwrap();
    let error = FileLock::acquire(&state.path(".lock"), Duration::from_millis(50)).unwrap_err();
    assert!(matches!(error, StateError::LockTimeout { .. }), "{error:?}");
}

#[test]
fn the_lock_is_released_when_its_guard_is_dropped() {
    let (_dir, state) = open();
    let held = FileLock::acquire(&state.path(".lock"), Duration::from_secs(1)).unwrap();
    drop(held);
    FileLock::acquire(&state.path(".lock"), Duration::from_millis(50)).unwrap();
}

#[test]
fn an_atomic_write_replaces_the_whole_file_and_leaves_no_temporary_file() {
    let (dir, state) = open();
    let path = state.path("state.json");
    atomic_write(&path, b"one").unwrap();
    atomic_write(&path, b"two").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"two");
    let leftovers: Vec<_> = fs::read_dir(state.root())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    drop(dir);
}

#[cfg(windows)]
#[test]
fn a_replacement_waits_for_a_reader_that_releases_the_file() {
    use std::os::windows::fs::OpenOptionsExt;

    /// `FILE_SHARE_READ`: other openers may read, but may not delete or replace the file.
    const FILE_SHARE_READ: u32 = 0x1;

    let (_dir, state) = open();
    let path = state.path("state.json");
    atomic_write(&path, b"old").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        drop(held);
    });
    atomic_write(&path, b"new").unwrap();
    releaser.join().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
}

#[test]
fn the_first_call_from_a_session_creates_one_task_and_later_calls_reuse_it() {
    let (_dir, state) = open();
    let first = state.bind_session("ses_one", "repo").unwrap();
    let again = state.bind_session("ses_one", "repo").unwrap();
    let other = state.bind_session("ses_two", "repo").unwrap();
    assert_eq!(first, "task-0001");
    assert_eq!(again, first);
    assert_eq!(other, "task-0002");
    assert_eq!(state.session_task("ses_one").unwrap(), Some(first));
    assert_eq!(state.session_task("ses_missing").unwrap(), None);
}

#[test]
fn a_session_id_that_is_not_an_identifier_is_refused_before_any_write() {
    let (_dir, state) = open();
    assert!(state.bind_session("../escape", "repo").is_err());
    assert!(!state.path("state.json").exists());
}

#[test]
fn task_appends_are_numbered_without_gaps_across_threads() {
    use harness_core::journal::Record;
    use std::sync::Arc;
    let (_dir, state) = open();
    let state = Arc::new(state);
    let id = state.bind_session("ses_threads", "repo").unwrap();
    let handles: Vec<_> = (0..4)
        .map(|thread_id| {
            let state = Arc::clone(&state);
            let id = id.clone();
            thread::spawn(move || {
                let task = state.task(&id).unwrap();
                for call in 0..15 {
                    thread::sleep(Duration::from_millis(5));
                    task.append(Record::CallStarted {
                        call_id: format!("t{thread_id}-c{call}"),
                        tool: "read".to_owned(),
                        read_only: true,
                        verification: false,
                        command_hash: None,
                        edited_file: None,
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let task = state.task(&id).unwrap();
    let entries = task.entries().unwrap();
    let seqs: Vec<u64> = entries.iter().map(|e| e.seq).collect();
    let expected: Vec<u64> = (1..=entries.len() as u64).collect();
    assert_eq!(seqs, expected, "sequence numbers must be contiguous");
    assert_eq!(
        entries.len(),
        1 + 4 * 15,
        "the TaskStarted record plus every call"
    );
    assert_eq!(task.snapshot().unwrap().last_seq, entries.len() as u64);
}

#[test]
fn a_cache_that_is_missing_is_rebuilt_from_the_journal() {
    use harness_core::journal::Record;
    let (dir, state) = open();
    let id = state.bind_session("ses_cache", "repo").unwrap();
    let task = state.task(&id).unwrap();
    task.append(Record::CallStarted {
        call_id: "c1".to_owned(),
        tool: "edit".to_owned(),
        read_only: false,
        verification: false,
        command_hash: None,
        edited_file: Some("calc.py".to_owned()),
    })
    .unwrap();
    let before = task.snapshot().unwrap();
    fs::remove_file(state.root().join("tasks").join(&id).join("task.json")).unwrap();
    let after = task.snapshot().unwrap();
    assert_eq!(before, after);
    drop(dir);
}
