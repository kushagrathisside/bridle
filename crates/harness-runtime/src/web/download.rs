//! Downloading a file for the person: streamed to disk (never held in memory), limited in size, checked by SHA-256 when the
//! expected value is known, sniffed for what it really is, and moved into place only when complete. An executable is refused: the
//! web tools fetch material to read, and nothing they download is ever run. This is a **network** operation with a fixed
//! destination, the project's `.bridle/web/downloads/`, so it needs no write permission to a path the model chose.
//! Not built yet: resuming an interrupted download (a partial file is removed), and unpacking archives (see the plan).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::fetch::{Fetcher, hex};

/// What the first bytes of a file say it is.
pub fn sniff(head: &[u8]) -> &'static str {
    let starts = |magic: &[u8]| head.starts_with(magic);
    if starts(b"%PDF-") {
        "pdf"
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        "zip"
    } else if starts(&[0x1f, 0x8b]) {
        "gzip"
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if starts(&[0xff, 0xd8, 0xff]) {
        "jpeg"
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        "gif"
    } else if starts(b"MZ") {
        "executable (Windows)"
    } else if starts(b"\x7fELF") {
        "executable (Linux)"
    } else if starts(&[0xcf, 0xfa, 0xed, 0xfe])
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])
        || starts(&[0xca, 0xfe, 0xba, 0xbe])
    {
        "executable (macOS)"
    } else if starts(b"#!") {
        "script"
    } else if starts(b"7z\xbc\xaf\x27\x1c") {
        "7z"
    } else if starts(b"Rar!") {
        "rar"
    } else if head
        .iter()
        .take(512)
        .all(|b| *b == b'\n' || *b == b'\r' || *b == b'\t' || (32..127).contains(b) || *b >= 128)
    {
        "text"
    } else {
        "binary"
    }
}

fn executable(kind: &str) -> bool {
    kind.starts_with("executable") || kind == "script"
}

/// A file name that is safe on every system, from the last part of a URL's path.
pub fn safe_name(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest
        .split_once('/')
        .map_or("", |(_, p)| p)
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let last = path
        .rsplit('/')
        .find(|p| !p.is_empty() && !p.contains(':'))
        .unwrap_or("download");
    let cleaned: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_matches('.').to_owned();
    if cleaned.is_empty() || cleaned.starts_with('-') {
        format!("download-{cleaned}")
    } else {
        cleaned
    }
}

#[derive(Debug, Clone)]
pub struct Downloaded {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub content_type: String,
    /// What the file is, from its first bytes.
    pub detected: String,
    pub final_url: String,
    /// True when the same file was already there and nothing new was written.
    pub already_there: bool,
}

pub fn download(
    fetcher: &Fetcher,
    url: &str,
    dest_dir: &Path,
    max_bytes: u64,
    expected_sha256: Option<&str>,
) -> Result<Downloaded, String> {
    let (response, final_url, _) = fetcher.follow(url, "*/*")?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(format!("{final_url} answered HTTP {status}"));
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if let Some(declared) = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && declared > max_bytes
    {
        return Err(format!(
            "the file is {declared} bytes, over the limit of {max_bytes}"
        ));
    }
    fs::create_dir_all(dest_dir).map_err(|e| format!("cannot create the download folder: {e}"))?;
    let name = safe_name(&final_url);
    let temp = dest_dir.join(format!(".part-{}-{name}", std::process::id()));
    let result = (|| -> Result<Downloaded, String> {
        let mut file =
            File::create(&temp).map_err(|e| format!("cannot create {}: {e}", temp.display()))?;
        let mut reader = response.into_body().into_reader();
        let mut digest = Sha256::new();
        let mut head: Vec<u8> = Vec::new();
        let mut total = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|e| format!("the download was interrupted: {e}"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max_bytes {
                return Err(format!(
                    "the file is larger than the limit of {max_bytes} bytes; stopped"
                ));
            }
            if head.len() < 512 {
                head.extend_from_slice(&buffer[..n.min(512 - head.len())]);
                if head.len() >= 4 {
                    let kind = sniff(&head);
                    if executable(kind) {
                        return Err(format!(
                            "the file is a {kind}; the web tools do not download programs"
                        ));
                    }
                }
            }
            digest.update(&buffer[..n]);
            file.write_all(&buffer[..n])
                .map_err(|e| format!("cannot write the file: {e}"))?;
        }
        file.sync_all()
            .map_err(|e| format!("cannot flush the file: {e}"))?;
        let sha256 = hex(&digest.finalize());
        if let Some(expected) = expected_sha256
            && !expected.eq_ignore_ascii_case(&sha256)
        {
            return Err(format!(
                "the checksum does not match: expected {expected}, got {sha256}"
            ));
        }
        let detected = sniff(&head);
        let lower = name.to_ascii_lowercase();
        for (extension, kind) in [
            (".pdf", "pdf"),
            (".zip", "zip"),
            (".png", "png"),
            (".jpg", "jpeg"),
            (".jpeg", "jpeg"),
            (".gz", "gzip"),
        ] {
            if lower.ends_with(extension) && detected != kind {
                return Err(format!(
                    "the file is named {name} but its content is {detected}, not {kind}; refused"
                ));
            }
        }
        let final_path = dest_dir.join(format!("{}-{name}", &sha256[..8]));
        let already_there = final_path.exists();
        if already_there {
            let _ = fs::remove_file(&temp);
        } else {
            fs::rename(&temp, &final_path)
                .map_err(|e| format!("cannot move the file into place: {e}"))?;
        }
        Ok(Downloaded {
            path: final_path,
            bytes: total,
            sha256,
            content_type: content_type.clone(),
            detected: detected.to_owned(),
            final_url: final_url.clone(),
            already_there,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::fetch::tests::serve;
    use super::*;
    use crate::http::RemoteRoute;
    use std::time::Duration;

    fn fetcher(port: u16) -> Fetcher {
        Fetcher::new(
            RemoteRoute::Direct,
            Duration::from_secs(5),
            1_000_000,
            &|_| None,
        )
        .exempting("127.0.0.1", port)
    }

    fn pdf() -> Vec<u8> {
        let mut v = b"%PDF-1.7\n".to_vec();
        v.extend(std::iter::repeat_n(b'x', 3_000));
        v
    }

    #[test]
    fn files_are_recognised_by_their_first_bytes() {
        assert_eq!(sniff(b"%PDF-1.4"), "pdf");
        assert_eq!(sniff(b"PK\x03\x04rest"), "zip");
        assert_eq!(sniff(b"MZ\x90\x00"), "executable (Windows)");
        assert_eq!(sniff(b"\x7fELF\x02"), "executable (Linux)");
        assert_eq!(sniff(b"#!/bin/sh\n"), "script");
        assert_eq!(sniff(b"plain text, nothing odd\n"), "text");
        assert_eq!(sniff(&[0u8, 1, 2, 3, 200, 9]), "binary");
    }

    #[test]
    fn names_from_addresses_are_safe_on_every_system() {
        assert_eq!(safe_name("https://e.org/a/paper.pdf?x=1#y"), "paper.pdf");
        assert_eq!(safe_name("https://e.org/"), "download");
        let evil = safe_name("https://e.org/..%2f..%2fevil");
        assert!(
            !evil.contains('/') && !evil.contains('%') && !evil.starts_with('.'),
            "{evil}"
        );
        assert!(safe_name("https://e.org/CON:").len() <= 80);
        assert!(safe_name(&format!("https://e.org/{}", "a".repeat(500))).len() <= 80);
    }

    #[test]
    fn a_download_is_streamed_checked_and_put_in_place_once() {
        let body = pdf();
        let port = serve(vec![(
            "/paper.pdf",
            200,
            vec![("Content-Type", "application/pdf".into())],
            body.clone(),
        )]);
        let dir = tempfile::tempdir().unwrap();
        let sha = hex(&Sha256::digest(&body));
        let got = download(
            &fetcher(port),
            &format!("http://127.0.0.1:{port}/paper.pdf"),
            dir.path(),
            1_000_000,
            Some(&sha),
        )
        .unwrap();
        assert_eq!(
            (got.bytes, got.sha256.as_str(), got.detected.as_str()),
            (body.len() as u64, sha.as_str(), "pdf")
        );
        assert_eq!(fs::read(&got.path).unwrap(), body);
        assert!(
            got.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&sha[..8])
        );
        let again = download(
            &fetcher(port),
            &format!("http://127.0.0.1:{port}/paper.pdf"),
            dir.path(),
            1_000_000,
            None,
        )
        .unwrap();
        assert!(
            again.already_there && again.path == got.path,
            "the same file is recognised and not written twice"
        );
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "no partial file is left");
    }

    #[test]
    fn a_wrong_checksum_a_big_file_an_executable_and_a_lying_name_are_all_refused_and_leave_nothing()
     {
        let exe = {
            let mut v = b"MZ\x90\x00".to_vec();
            v.extend(std::iter::repeat_n(0u8, 2_000));
            v
        };
        let port = serve(vec![
            ("/paper.pdf", 200, vec![], pdf()),
            ("/big.bin", 200, vec![], vec![7u8; 50_000]),
            ("/tool.exe", 200, vec![], exe.clone()),
            ("/fake.pdf", 200, vec![], exe),
            (
                "/page.pdf",
                200,
                vec![],
                b"<html>this is not a pdf</html>".to_vec(),
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let f = fetcher(port);
        let url = |p: &str| format!("http://127.0.0.1:{port}{p}");
        assert!(
            download(
                &f,
                &url("/paper.pdf"),
                dir.path(),
                1_000_000,
                Some(&"0".repeat(64))
            )
            .unwrap_err()
            .contains("checksum does not match")
        );
        assert!(
            download(&f, &url("/big.bin"), dir.path(), 10_000, None)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            download(&f, &url("/tool.exe"), dir.path(), 1_000_000, None)
                .unwrap_err()
                .contains("do not download programs")
        );
        assert!(
            download(&f, &url("/fake.pdf"), dir.path(), 1_000_000, None)
                .unwrap_err()
                .contains("do not download programs")
        );
        assert!(
            download(&f, &url("/page.pdf"), dir.path(), 1_000_000, None)
                .unwrap_err()
                .contains("not pdf")
        );
        assert!(
            download(&f, &url("/missing"), dir.path(), 1_000_000, None)
                .unwrap_err()
                .contains("404")
        );
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "nothing is left behind: {left:?}");
    }
}
