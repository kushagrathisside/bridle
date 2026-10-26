//! File primitives for the state store: atomic replacement, stale temporary files and the lock.
//!
//! Windows reports a sharing violation when a rename replaces a file that another process has open
//! (antivirus, the search indexer, an editor). The rename is therefore retried a few times within
//! the hook timeout before it is reported as a failure.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::StateError;

/// Rename attempts before a replacement is reported as failed.
const RENAME_TRIES: u32 = 5;
/// Pause between rename attempts. Five tries take about 240 ms, inside the hook timeout.
const RENAME_WAIT: Duration = Duration::from_millis(60);
/// Pause between attempts to take the lock.
const LOCK_POLL: Duration = Duration::from_millis(10);
/// A temporary file older than this was left by a killed process and is removed at open.
const STALE_TEMP_AGE: Duration = Duration::from_secs(60);
/// Marks a temporary file name. The name is the target file name, this marker, and a unique suffix.
const TEMP_MARKER: &str = ".tmp-";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes`. The content is written and synced to a temporary file in the same
/// directory, then renamed over `path`. Readers see either the old content or the new content.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let temp = temp_path(path);
    let result = write_and_sync(&temp, bytes)
        .map_err(|source| StateError::Io {
            action: "write",
            path: temp.clone(),
            source,
        })
        .and_then(|()| rename_with_retry(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Removes temporary files in `dir` that are older than [`STALE_TEMP_AGE`]. Removal is best effort:
/// a file that cannot be removed is left for the next open.
pub fn remove_stale_temps(dir: &Path) -> Result<(), StateError> {
    let entries = fs::read_dir(dir).map_err(|source| StateError::Io {
        action: "list",
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().contains(TEMP_MARKER) {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified());
        let stale = modified
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= STALE_TEMP_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// An exclusive lock on a lock file. The lock is released when the guard is dropped.
#[derive(Debug)]
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// Takes the lock on `path`, polling until it is free or `timeout` has passed.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, StateError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|source| StateError::Io {
                action: "open",
                path: path.to_path_buf(),
                source,
            })?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) => {
                    if started.elapsed() >= timeout {
                        return Err(StateError::LockTimeout {
                            path: path.to_path_buf(),
                            timeout_ms: timeout.as_millis(),
                        });
                    }
                    thread::sleep(LOCK_POLL);
                }
                Err(TryLockError::Error(source)) => {
                    return Err(StateError::Io {
                        action: "lock",
                        path: path.to_path_buf(),
                        source,
                    });
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        "{name}{TEMP_MARKER}{}-{unique}",
        std::process::id()
    ))
}

fn write_and_sync(temp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn rename_with_retry(from: &Path, to: &Path) -> Result<(), StateError> {
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(source) if attempt >= RENAME_TRIES => {
                return Err(StateError::Io {
                    action: "replace",
                    path: to.to_path_buf(),
                    source,
                });
            }
            Err(_) => {
                attempt += 1;
                thread::sleep(RENAME_WAIT);
            }
        }
    }
}
