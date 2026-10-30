//! Persistent task state under `.local-agent/`.
//!
//! Every file is a versioned envelope, `{"schema": 1, "data": ...}`. Readers tolerate unknown fields
//! inside `data`, refuse a newer schema by name, and skip a torn last line of a JSON Lines file.
//! Writers redact every string before it reaches the disk. Whole-file writes go through an atomic
//! rename, and appends and cache writes go through the state lock, so a killed process leaves the
//! previous content in place.

mod fs;
mod jsonl;
mod task;

use std::fs as std_fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::identity;
use harness_core::security::redact::Redactor;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

pub use fs::{FileLock, atomic_write};
pub use jsonl::{JsonlRead, read_records};
pub use task::{
    CACHE_FILE, DECISIONS_FILE, DecisionRecord, INDEX_FILE, JOURNAL_FILE, MCP_RESULTS_FILE,
    McpResult, SessionIndex, TASKS_DIR, Task, task_dir, validate_session_id, validate_task_id,
};

/// The schema version this build reads and writes. Older versions are migrated on read, here.
pub const SCHEMA_VERSION: u32 = 1;
/// How long a writer waits for the state lock before it reports a timeout.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// Name of the lock file inside the state directory.
pub const LOCK_FILE: &str = ".lock";

/// Every failure the state store reports. Each variant names the file it concerns.
#[derive(Debug, Error)]
pub enum StateError {
    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{path} was written with schema {found}; this build reads schema {supported} and older. Upgrade the harness."
    )]
    NewerSchema {
        path: PathBuf,
        found: u64,
        supported: u32,
    },
    #[error("{path} is not valid state: {message}")]
    Corrupt { path: PathBuf, message: String },
    #[error("timed out after {timeout_ms} ms waiting for the state lock {path}")]
    LockTimeout { path: PathBuf, timeout_ms: u128 },
    #[error("invalid {what} {value:?}: {rule}")]
    InvalidId {
        what: &'static str,
        value: String,
        rule: &'static str,
    },
}

/// The state directory of one workspace: `<workspace>/.local-agent/`.
#[derive(Debug)]
pub struct StateDir {
    root: PathBuf,
    redactor: Redactor,
}

impl StateDir {
    /// Opens the state directory of `workspace`, creating it if needed, and removes temporary files
    /// that a killed process left behind.
    pub fn open(workspace: &Path) -> Result<Self, StateError> {
        let root = workspace.join(identity::STATE_DIR);
        std_fs::create_dir_all(&root).map_err(|source| StateError::Io {
            action: "create",
            path: root.clone(),
            source,
        })?;
        fs::remove_stale_temps(&root)?;
        Ok(Self {
            root,
            redactor: Redactor::new(),
        })
    }

    /// The state directory of `workspace` for reading only: it is not created, no temporary file
    /// is removed and no lock is taken. `None` when the workspace has no state directory yet.
    /// For viewers that must not change anything; writers use [`StateDir::open`].
    pub fn open_existing(workspace: &Path) -> Option<Self> {
        let root = workspace.join(identity::STATE_DIR);
        root.is_dir().then(|| Self {
            root,
            redactor: Redactor::new(),
        })
    }

    /// The state directory itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path of a file inside the state directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Takes the state lock, waiting up to [`LOCK_TIMEOUT`].
    pub fn lock(&self) -> Result<FileLock, StateError> {
        FileLock::acquire(&self.path(LOCK_FILE), LOCK_TIMEOUT)
    }

    /// Replaces the whole content of `name` with `data`, atomically and redacted.
    pub fn write_json<T: Serialize>(&self, name: &str, data: &T) -> Result<(), StateError> {
        let path = self.path(name);
        ensure_parent(&path)?;
        let mut value = envelope(&path, data)?;
        self.redactor.redact_json(&mut value);
        let bytes = serde_json::to_vec_pretty(&value).map_err(|e| corrupt(&path, e))?;
        atomic_write(&path, &bytes)
    }

    /// Reads `name`. Returns `None` when the file does not exist.
    pub fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, StateError> {
        let path = self.path(name);
        let text = match std_fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StateError::Io {
                    action: "read",
                    path,
                    source,
                });
            }
        };
        let value: Value = serde_json::from_str(&text).map_err(|e| corrupt(&path, e))?;
        let data = open_envelope(&path, value)?;
        serde_json::from_value(data)
            .map(Some)
            .map_err(|e| corrupt(&path, e))
    }

    /// Appends one record to the JSON Lines file `name`, redacted, under the state lock.
    pub fn append<T: Serialize>(&self, name: &str, record: &T) -> Result<(), StateError> {
        let _guard = self.lock()?;
        self.append_unlocked(name, record)
    }

    /// Appends without taking the workspace lock. The caller holds a lock that covers the file.
    pub(crate) fn append_unlocked<T: Serialize>(
        &self,
        name: &str,
        record: &T,
    ) -> Result<(), StateError> {
        let path = self.path(name);
        ensure_parent(&path)?;
        jsonl::append_record(&path, record, &self.redactor)
    }

    /// Reads every record of the JSON Lines file `name`. A missing file is an empty log.
    pub fn read_records<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<JsonlRead<T>, StateError> {
        jsonl::read_records(&self.path(name))
    }
}

/// Creates the directory that will hold `path`, for per-task files.
fn ensure_parent(path: &Path) -> Result<(), StateError> {
    match path.parent() {
        Some(parent) => std_fs::create_dir_all(parent).map_err(|source| StateError::Io {
            action: "create",
            path: parent.to_path_buf(),
            source,
        }),
        None => Ok(()),
    }
}

/// Wraps `data` in the schema envelope.
pub(crate) fn envelope<T: Serialize>(path: &Path, data: &T) -> Result<Value, StateError> {
    let data = serde_json::to_value(data).map_err(|e| corrupt(path, e))?;
    Ok(serde_json::json!({ "schema": SCHEMA_VERSION, "data": data }))
}

/// Checks the schema version of an envelope and returns its `data`.
pub(crate) fn open_envelope(path: &Path, mut value: Value) -> Result<Value, StateError> {
    let found = value
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or_else(|| StateError::Corrupt {
            path: path.to_path_buf(),
            message: "no schema version".to_owned(),
        })?;
    if found > u64::from(SCHEMA_VERSION) {
        return Err(StateError::NewerSchema {
            path: path.to_path_buf(),
            found,
            supported: SCHEMA_VERSION,
        });
    }
    value
        .get_mut("data")
        .map(Value::take)
        .ok_or_else(|| StateError::Corrupt {
            path: path.to_path_buf(),
            message: "no data".to_owned(),
        })
}

/// A `Corrupt` error for a file whose content does not parse.
pub(crate) fn corrupt(path: &Path, error: impl std::fmt::Display) -> StateError {
    StateError::Corrupt {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_round_trips_through_open_envelope() {
        let path = Path::new("state.json");
        let wrapped = envelope(path, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(wrapped["schema"], SCHEMA_VERSION);
        assert_eq!(
            open_envelope(path, wrapped).unwrap(),
            serde_json::json!({"a": 1})
        );
    }

    #[test]
    fn a_newer_schema_is_refused_by_name() {
        let value = serde_json::json!({"schema": SCHEMA_VERSION + 1, "data": {}});
        let error = open_envelope(Path::new("task.json"), value).unwrap_err();
        assert!(
            matches!(error, StateError::NewerSchema { found, .. } if found == u64::from(SCHEMA_VERSION) + 1)
        );
        assert!(error.to_string().contains("Upgrade the harness"));
    }

    #[test]
    fn a_missing_schema_is_corrupt() {
        let error =
            open_envelope(Path::new("task.json"), serde_json::json!({"data": {}})).unwrap_err();
        assert!(matches!(error, StateError::Corrupt { .. }));
    }
}
