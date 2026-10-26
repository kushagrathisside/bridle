//! Append-only JSON Lines files for the state store.
//!
//! Each line is one versioned envelope. A line is written with a single `write_all` in append mode,
//! so a record is never interleaved with another. A process killed mid-write can leave a last line
//! without its newline; the reader skips that line and counts it. A damaged line in the middle of a
//! file is not skipped: it is reported with its line number, because it cannot be a torn write.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

use harness_core::security::redact::Redactor;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{StateError, corrupt, envelope, open_envelope};

/// The records of a JSON Lines file, and how many torn last lines were skipped.
#[derive(Debug)]
pub struct JsonlRead<T> {
    pub records: Vec<T>,
    pub torn_lines: usize,
}

/// Appends one redacted record. The caller holds the state lock.
pub(crate) fn append_record<T: Serialize>(
    path: &Path,
    record: &T,
    redactor: &Redactor,
) -> Result<(), StateError> {
    let mut value = envelope(path, record)?;
    redactor.redact_json(&mut value);
    let mut line = serde_json::to_string(&value).map_err(|e| corrupt(path, e))?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
        .map_err(|source| StateError::Io {
            action: "open",
            path: path.to_path_buf(),
            source,
        })?;
    drop_torn_tail(path, &mut file)?;
    file.write_all(line.as_bytes())
        .map_err(|source| StateError::Io {
            action: "append",
            path: path.to_path_buf(),
            source,
        })
}

/// Reads every record of the file at `path`. A missing file is an empty log.
pub fn read_records<T: DeserializeOwned>(path: &Path) -> Result<JsonlRead<T>, StateError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(JsonlRead {
                records: Vec::new(),
                torn_lines: 0,
            });
        }
        Err(source) => {
            return Err(StateError::Io {
                action: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    // Without a final newline, the last line may be a write that was never finished.
    let complete = text.ends_with('\n');
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .collect();
    let last = lines.last().map(|(index, _)| *index);

    let mut records = Vec::with_capacity(lines.len());
    let mut torn_lines = 0;
    for (index, line) in lines {
        match parse_line::<T>(path, line) {
            Ok(record) => records.push(record),
            Err(StateError::Corrupt { .. }) if !complete && Some(index) == last => {
                torn_lines += 1;
            }
            Err(StateError::Corrupt { message, .. }) => {
                return Err(StateError::Corrupt {
                    path: path.to_path_buf(),
                    message: format!("line {}: {message}", index + 1),
                });
            }
            Err(other) => return Err(other),
        }
    }
    Ok(JsonlRead {
        records,
        torn_lines,
    })
}

/// Removes a last line that a killed process left without its newline. That line was never committed.
/// Keeping it would put the next record on the same line, which is a damaged middle line on the next read.
fn drop_torn_tail(path: &Path, file: &mut File) -> Result<(), StateError> {
    let io = |action: &'static str| {
        let path = path.to_path_buf();
        move |source: std::io::Error| StateError::Io {
            action,
            path: path.clone(),
            source,
        }
    };
    let len = file.metadata().map_err(io("inspect"))?.len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1)).map_err(io("read"))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last).map_err(io("read"))?;
    if last[0] == b'\n' {
        return Ok(());
    }
    file.seek(SeekFrom::Start(0)).map_err(io("read"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io("read"))?;
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    file.set_len(u64::try_from(keep).unwrap_or(0))
        .map_err(io("truncate"))
}

fn parse_line<T: DeserializeOwned>(path: &Path, line: &str) -> Result<T, StateError> {
    let value: Value = serde_json::from_str(line).map_err(|e| corrupt(path, e))?;
    let data = open_envelope(path, value)?;
    serde_json::from_value(data).map_err(|e| corrupt(path, e))
}
