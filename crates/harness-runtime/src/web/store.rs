//! The record of every source the web tools have read: an address, what it finally was, when it was read, and the SHA-256 of what
//! was read. It is an append-only file in the project's state folder, so a report can cite `S-0007` and the claim can be traced back
//! to the page as it was. Written under a file lock, one JSON object per line.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use harness_core::identity;
use serde::{Deserialize, Serialize};
use sha2::Digest;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// What the record is: `search_result`, `page`, `download` or `screenshot`.
    pub kind: String,
    /// How the content was obtained: `http` (the plain fetcher, also the meaning of a record that has no such field) or `browser`
    /// (the managed Chromium, after scripts ran).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extraction: String,
    /// For a browser record: the browser and Playwright that produced it, and the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playwright_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// For a browser record of a page that was also read over plain HTTP: the id of that earlier record. The two are different
    /// readings of one address (the rendered page and the page as sent), so both are kept, and this says they belong together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendition_of: Option<String>,
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

    /// Where screenshots from the browser are kept.
    pub fn screenshots_dir(&self) -> PathBuf {
        self.dir.join("screenshots")
    }

    /// Keeps a PNG picture: checks the signature and the size, names it by its hash, copies it into the screenshot folder (atomically,
    /// once), and returns the final path, the SHA-256 and the length. The source file is not touched.
    pub fn keep_png(
        &self,
        source: &Path,
        max_bytes: u64,
    ) -> Result<(PathBuf, String, u64), String> {
        use std::io::Read;
        let meta =
            fs::symlink_metadata(source).map_err(|e| format!("cannot read the picture: {e}"))?;
        if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > max_bytes {
            return Err(format!(
                "the picture is not a file of up to {max_bytes} bytes"
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(source)
            .and_then(|f| f.take(max_bytes + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("cannot read the picture: {e}"))?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("the picture is not a PNG".to_owned());
        }
        let sha = crate::web::fetch::hex(&sha2::Sha256::digest(&bytes));
        let dir = self.screenshots_dir();
        fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create the screenshot folder: {e}"))?;
        let final_path = dir.join(format!("{}-window.png", &sha[..8]));
        if !final_path.exists() {
            let temp = dir.join(format!(".part-{}-window.png", std::process::id()));
            fs::write(&temp, &bytes).map_err(|e| format!("cannot save the picture: {e}"))?;
            fs::rename(&temp, &final_path).map_err(|e| format!("cannot save the picture: {e}"))?;
        }
        Ok((final_path, sha, bytes.len() as u64))
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
            r.final_url == record.final_url
                && r.sha256 == record.sha256
                && r.kind == record.kind
                && r.extraction == record.extraction
        }) {
            return Ok(found.clone());
        }
        if record.extraction == "browser" && record.kind == "page" && record.rendition_of.is_none()
        {
            record.rendition_of = existing
                .iter()
                .find(|r| {
                    r.kind == "page" && r.extraction != "browser" && r.final_url == record.final_url
                })
                .map(|r| r.id.clone());
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
            ..SourceRecord::default()
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

    #[test]
    fn a_browser_reading_of_a_page_links_to_the_plain_one_and_is_not_recorded_twice() {
        let dir = tempfile::tempdir().unwrap();
        let s = Sources::open(dir.path());
        let plain = s.record(rec("https://a.org/", "aa")).unwrap();
        let browser = |sha: &str| SourceRecord {
            extraction: "browser".into(),
            browser_version: Some("156".into()),
            playwright_version: Some("1.64.1".into()),
            session: Some("br-1".into()),
            ..rec("https://a.org/", sha)
        };
        let rendered = s.record(browser("bb")).unwrap();
        assert_ne!(rendered.id, plain.id);
        assert_eq!(rendered.rendition_of.as_deref(), Some(plain.id.as_str()));
        // The same rendering again is the same record; a different one is new.
        assert_eq!(s.record(browser("bb")).unwrap().id, rendered.id);
        assert_ne!(s.record(browser("cc")).unwrap().id, rendered.id);
        // A page nobody read over HTTP has nothing to link to.
        let alone = s
            .record(SourceRecord {
                extraction: "browser".into(),
                ..rec("https://b.org/", "dd")
            })
            .unwrap();
        assert_eq!(alone.rendition_of, None);
        // Old records, written before these fields, still read.
        let old = r#"{"id":"S-0009","url":"u","final_url":"u","title":"","provider":"","content_type":"","sha256":"x","bytes":1,"retrieved_at":"t","kind":"page"}"#;
        let parsed: SourceRecord = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.extraction, "");
    }
}
