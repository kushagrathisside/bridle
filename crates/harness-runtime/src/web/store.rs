//! The record of every source the web tools have read: an address, what it finally was, when it was read, and the SHA-256 of what
//! was read. It is an append-only file in the project's state folder, so a report can cite `S-0007` and the claim can be traced back
//! to the page as it was. Written under a file lock, one JSON object per line.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use harness_core::identity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: String,
    pub url: String,
    pub final_url: String,
    pub title: String,
    pub provider: String,
    pub content_type: String,
    pub sha256: String,
    pub bytes: u64,
    pub retrieved_at: String,
    /// What the record is: `search_result`, `page` or `download`.
    pub kind: String,
}

#[derive(Debug)]
pub struct Sources {
    dir: PathBuf,
}

impl Sources {
    pub fn open(workspace: &Path) -> Self {
        Self {
            dir: identity::state_dir(workspace).join("web"),
        }
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.dir.join("downloads")
    }

    fn file(&self) -> PathBuf {
        self.dir.join("sources.jsonl")
    }

    /// Adds a record and gives it the next id (`S-0001`, `S-0002`, ...). A record for the same final address and checksum that
    /// already exists is returned instead of being added again.
    pub fn record(&self, mut record: SourceRecord) -> std::io::Result<SourceRecord> {
        fs::create_dir_all(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("sources.lock"))?;
        lock.lock()?;
        let existing = self.list();
        if let Some(found) = existing.iter().find(|r| {
            r.final_url == record.final_url && r.sha256 == record.sha256 && r.kind == record.kind
        }) {
            return Ok(found.clone());
        }
        record.id = format!("S-{:04}", existing.len() + 1);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file())?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_default()
        )?;
        file.sync_all()?;
        drop(lock);
        Ok(record)
    }

    /// Every record, oldest first. A damaged line is skipped.
    pub fn list(&self) -> Vec<SourceRecord> {
        fs::read_to_string(self.file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

pub fn now() -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn rec(url: &str, sha: &str) -> SourceRecord {
        SourceRecord {
            id: String::new(),
            url: url.into(),
            final_url: url.into(),
            title: "t".into(),
            provider: "p".into(),
            content_type: "text/html".into(),
            sha256: sha.into(),
            bytes: 1,
            retrieved_at: now(),
            kind: "page".into(),
        }
    }

    #[test]
    fn records_get_sequential_ids_and_the_same_page_is_not_recorded_twice() {
        let dir = tempfile::tempdir().unwrap();
        let s = Sources::open(dir.path());
        let a = s.record(rec("https://a.org/", "aa")).unwrap();
        let b = s.record(rec("https://b.org/", "bb")).unwrap();
        let again = s.record(rec("https://a.org/", "aa")).unwrap();
        let changed = s.record(rec("https://a.org/", "cc")).unwrap();
        assert_eq!(
            (
                a.id.as_str(),
                b.id.as_str(),
                again.id.as_str(),
                changed.id.as_str()
            ),
            ("S-0001", "S-0002", "S-0001", "S-0003")
        );
        assert_eq!(s.list().len(), 3);
        // A damaged line does not hide the others.
        fs::write(
            s.file(),
            format!("{}\nnot json\n", serde_json::to_string(&a).unwrap()),
        )
        .unwrap();
        assert_eq!(s.list().len(), 1);
    }
}
