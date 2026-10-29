//! Model manifests: identity, revision, quantisation, hash, runtime build and hardware,
//! persisted so benchmark results are comparable.
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `sha256_file(path)`: streaming SHA-256 (1 MiB buffer) → lowercase hex. Use `sha2` 0.11
//!   (`Sha256::new()`, `update`, `finalize()`; format bytes as hex manually).
//! - `HashCache` at `<state_dir>/manifests/hash-cache.json`: map of absolute path → `{size,
//!   modified_unix_ms, sha256}`; `cached_sha256(cache_path, file)` returns the cached hash when
//!   size and mtime match, otherwise hashes, stores and returns. Hashing a 6 GB file takes tens
//!   of seconds; the cache makes `agent doctor` fast on repeat runs.
//! - `parse_llama_version(stdout)`: llama.cpp `--version` prints lines like
//!   `version: 11756 (a1b2c3d)` and `built with ... for x86_64`; return
//!   `Some(RuntimeVersion { build: "11756", commit: Some("a1b2c3d") })`. Prism builds print the same
//!   shape. Tolerate `\r\n`. Unit-test with both shapes and garbage.
//! - `build_manifest(input: ManifestInput) -> ModelManifest`: pure assembly from the pieces the
//!   CLI gathers (slot, ModelConfig, resolved file path, sha256, runtime kind/build/commit,
//!   GPU/driver/CUDA strings, OS). `verified` = expected sha256 (from `model.source.sha256`) is
//!   `None` → `None`; equal (case-insensitive) → `Some(true)`; different → `Some(false)`.
//! - `write_manifest(dir, &ModelManifest)` → `<dir>/<slot>.json` pretty JSON; returns the path.
//! - Tests: hashing a temp file against a known digest ("abc" →
//!   `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`), cache hit/miss on mtime
//!   change, verified flag, JSON round trip.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use harness_core::config::ModelConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Read buffer for hashing: 1 MiB keeps memory flat on multi-gigabyte weight files.
const HASH_BUFFER_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub build: String,
    pub commit: Option<String>,
}

/// Everything recorded about one model slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    pub slot: String,
    pub model: String,
    pub quantization: Option<String>,
    pub source: Option<String>,
    pub revision: Option<String>,
    pub file: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub expected_sha256: Option<String>,
    pub verified: Option<bool>,
    pub license: Option<String>,
    pub runtime: Option<String>,
    pub runtime_build: Option<String>,
    pub runtime_commit: Option<String>,
    pub gpu: Option<String>,
    pub driver: Option<String>,
    pub cuda: Option<String>,
    pub os: String,
    pub created_at: String,
}

/// Inputs gathered by the caller (the CLI probes the system; this module stays deterministic).
#[derive(Debug, Clone)]
pub struct ManifestInput<'a> {
    pub slot: &'a str,
    pub model: &'a ModelConfig,
    pub model_file: Option<&'a Path>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub runtime_kind: Option<String>,
    pub runtime: Option<RuntimeVersion>,
    pub gpu: Option<String>,
    pub driver: Option<String>,
    pub cuda: Option<String>,
    pub os: String,
}

/// One entry of the hash cache: the digest is valid only while size and mtime still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub size: u64,
    pub modified_unix_ms: u64,
    pub sha256: String,
}

/// Hash cache keyed by the absolute path of each file.
pub type HashCache = BTreeMap<String, CacheEntry>;

/// Location of the hash cache under a state directory: `<state_dir>/manifests/hash-cache.json`.
pub fn hash_cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("manifests").join("hash-cache.json")
}

/// Streaming SHA-256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUFFER_BYTES];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// SHA-256 of `file`, reusing the cached digest when the file's size and mtime are unchanged.
///
/// Misses hash the file and store the result in `cache_path`. The cache only saves time, so a
/// missing, corrupt or unwritable cache falls back to hashing and never fails the call.
pub fn cached_sha256(cache_path: &Path, file: &Path) -> io::Result<String> {
    let meta = fs::metadata(file)?;
    let size = meta.len();
    let modified_unix_ms = unix_millis(meta.modified()?);
    let key = std::path::absolute(file)?.to_string_lossy().into_owned();

    let mut cache = read_cache(cache_path);
    if let Some(entry) = cache
        .get(&key)
        .filter(|e| e.size == size && e.modified_unix_ms == modified_unix_ms)
    {
        return Ok(entry.sha256.clone());
    }

    let sha256 = sha256_file(file)?;
    cache.insert(
        key,
        CacheEntry {
            size,
            modified_unix_ms,
            sha256: sha256.clone(),
        },
    );
    let _ = write_cache(cache_path, &cache);
    Ok(sha256)
}

/// Parses `llama-server --version` output. Accepts the current shape
/// (`version: 0.7.0-dev (build 11756, commit d80619397)`), the older shape
/// (`version: 11756 (a1b2c3d)`) and `\r\n` line endings. Returns `None` for anything else.
pub fn parse_llama_version(stdout: &str) -> Option<RuntimeVersion> {
    stdout.lines().find_map(parse_version_line)
}

fn parse_version_line(line: &str) -> Option<RuntimeVersion> {
    let rest = line.trim().split_once("version:")?.1.trim();
    let (head, inside) = match rest.split_once('(') {
        Some((head, tail)) => {
            let inner = tail.split_once(')').map_or(tail, |(inner, _)| inner);
            (head.trim(), inner.trim())
        }
        None => (rest, ""),
    };
    let fields: Vec<&str> = inside.split(',').map(str::trim).collect();
    let (build, commit) = match fields.iter().find_map(|f| labelled(f, "build")) {
        // Current shape: `(build 11756, commit d80619397)`.
        Some(build) => (build, fields.iter().find_map(|f| labelled(f, "commit"))),
        // Older shape: `11756 (a1b2c3d)`.
        None => (head.split_whitespace().next()?, Some(inside)),
    };
    if build.is_empty() || !build.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let commit = commit.filter(|c| !c.is_empty()).map(str::to_owned);
    Some(RuntimeVersion {
        build: build.to_owned(),
        commit,
    })
}

/// Value of a `label value` field, such as `build 11756`.
fn labelled<'a>(field: &'a str, label: &str) -> Option<&'a str> {
    let (name, value) = field.split_once(char::is_whitespace)?;
    (name == label).then(|| value.trim())
}

/// Assembles a manifest from the pieces the caller gathered. Pure: it reads the clock only for
/// `created_at`.
pub fn build_manifest(input: ManifestInput<'_>) -> ModelManifest {
    let ManifestInput {
        slot,
        model,
        model_file,
        size_bytes,
        sha256,
        runtime_kind,
        runtime,
        gpu,
        driver,
        cuda,
        os,
    } = input;
    let source = model.source.as_ref();
    let expected_sha256 = source.and_then(|s| s.sha256.clone());
    // Unknown when either digest is missing: an unhashed file is not a mismatch.
    let verified = match (&expected_sha256, &sha256) {
        (Some(expected), Some(actual)) => Some(expected.eq_ignore_ascii_case(actual)),
        _ => None,
    };
    ModelManifest {
        slot: slot.to_owned(),
        model: model.id.clone(),
        quantization: source.map(|s| s.quantization.clone()),
        source: source.map(|s| s.repo.clone()),
        revision: source.map(|s| s.revision.clone()),
        file: model_file.map(|p| p.display().to_string()),
        size_bytes,
        sha256,
        expected_sha256,
        verified,
        license: source.and_then(|s| s.license.clone()),
        runtime: runtime_kind,
        runtime_build: runtime.as_ref().map(|r| r.build.clone()),
        runtime_commit: runtime.and_then(|r| r.commit),
        gpu,
        driver,
        cuda,
        os,
        created_at: humantime::format_rfc3339_millis(SystemTime::now()).to_string(),
    }
}

/// Writes `manifest` as pretty JSON to `<dir>/<slot>.json` and returns that path.
pub fn write_manifest(dir: &Path, manifest: &ModelManifest) -> io::Result<PathBuf> {
    if manifest.slot.is_empty() || manifest.slot.contains(std::path::is_separator) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("slot name {:?} cannot be a file name", manifest.slot),
        ));
    }
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", manifest.slot));
    let mut json = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
    json.push('\n');
    fs::write(&path, json)?;
    Ok(path)
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Missing or unreadable cache files yield an empty cache.
fn read_cache(path: &Path) -> HashCache {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_cache(path: &Path, cache: &HashCache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    // Write beside the target and rename, so a crash never leaves a half-written cache.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    /// Digest of the bytes `abc`.
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    /// Current-release output captured from the Windows llama-server binary (CRLF endings).
    const LLAMA_CURRENT: &str = "version: 0.7.0-dev (build 11756, commit d80619397)\r\nbuilt with Clang 20.1.8 for Windows x86_64\r\n";
    /// Output of the PrismML fork (CRLF endings).
    const PRISM_CURRENT: &str = "version: 0.2.0-dev (build 10787, commit 73bfe6cc4)\r\nbuilt with MSVC 19.44.35229.0 for Windows AMD64\r\n";

    fn model(source: Option<serde_json::Value>) -> ModelConfig {
        let mut value = json!({
            "provider": "llama_cpp",
            "id": "qwen-main",
            "role": "engineer",
            "endpoint": "http://127.0.0.1:8001/v1",
            "context": 8192,
            "hardware": "gpu",
        });
        if let Some(source) = source {
            value["source"] = source;
        }
        serde_json::from_value(value).expect("test model config is valid")
    }

    fn source_with_sha(sha256: Option<&str>) -> serde_json::Value {
        json!({
            "repo": "unsloth/Qwen3.5-9B-GGUF",
            "revision": "abc123",
            "file": "Qwen3.5-9B-Q4_K_M.gguf",
            "quantization": "Q4_K_M",
            "sha256": sha256,
            "license": "apache-2.0",
        })
    }

    fn input<'a>(model: &'a ModelConfig, sha256: Option<String>) -> ManifestInput<'a> {
        ManifestInput {
            slot: "main",
            model,
            model_file: None,
            size_bytes: Some(3),
            sha256,
            runtime_kind: Some("llama_cpp".to_owned()),
            runtime: None,
            gpu: None,
            driver: None,
            cuda: None,
            os: "linux".to_owned(),
        }
    }

    #[test]
    fn sha256_of_abc_matches_known_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("abc.bin");
        fs::write(&file, b"abc").expect("write");
        assert_eq!(sha256_file(&file).expect("hash"), ABC_SHA256);
    }

    #[test]
    fn sha256_of_empty_file_matches_known_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("empty.bin");
        fs::write(&file, b"").expect("write");
        assert_eq!(
            sha256_file(&file).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_streams_files_larger_than_the_buffer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("big.bin");
        let bytes = vec![0x5au8; HASH_BUFFER_BYTES * 2 + 17];
        fs::write(&file, &bytes).expect("write");
        let expected: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha256_file(&file).expect("hash"), expected);
    }

    #[test]
    fn cache_miss_then_hit_then_miss_on_mtime_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("model.bin");
        fs::write(&file, b"abc").expect("write");
        let cache = hash_cache_path(&dir.path().join("state"));

        // Miss: hashes the file and stores the entry.
        assert_eq!(cached_sha256(&cache, &file).expect("miss"), ABC_SHA256);

        // Hit: a planted digest with matching size and mtime is returned as-is.
        let key = std::path::absolute(&file)
            .expect("absolute")
            .to_string_lossy()
            .into_owned();
        let mut map: HashCache =
            serde_json::from_slice(&fs::read(&cache).expect("cache exists")).expect("cache json");
        map.get_mut(&key).expect("entry stored").sha256 = "planted".to_owned();
        fs::write(&cache, serde_json::to_vec(&map).expect("serialize")).expect("write cache");
        assert_eq!(cached_sha256(&cache, &file).expect("hit"), "planted");

        // Miss again: a changed mtime invalidates the planted entry.
        let handle = File::options().write(true).open(&file).expect("open");
        handle
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .expect("set mtime");
        assert_eq!(cached_sha256(&cache, &file).expect("re-miss"), ABC_SHA256);
    }

    #[test]
    fn cache_recovers_from_a_corrupt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("model.bin");
        fs::write(&file, b"abc").expect("write");
        let cache = dir.path().join("hash-cache.json");
        fs::write(&cache, b"{not json").expect("write garbage");
        assert_eq!(cached_sha256(&cache, &file).expect("hash"), ABC_SHA256);
    }

    #[test]
    fn parses_current_llama_server_output_with_crlf() {
        assert_eq!(
            parse_llama_version(LLAMA_CURRENT),
            Some(RuntimeVersion {
                build: "11756".to_owned(),
                commit: Some("d80619397".to_owned()),
            })
        );
    }

    #[test]
    fn parses_current_prism_output_with_crlf() {
        assert_eq!(
            parse_llama_version(PRISM_CURRENT),
            Some(RuntimeVersion {
                build: "10787".to_owned(),
                commit: Some("73bfe6cc4".to_owned()),
            })
        );
    }

    #[test]
    fn parses_older_version_shape() {
        assert_eq!(
            parse_llama_version("version: 11756 (a1b2c3d)\nbuilt with gcc for x86_64\n"),
            Some(RuntimeVersion {
                build: "11756".to_owned(),
                commit: Some("a1b2c3d".to_owned()),
            })
        );
        assert_eq!(
            parse_llama_version("version: 11756\r\n"),
            Some(RuntimeVersion {
                build: "11756".to_owned(),
                commit: None,
            })
        );
    }

    #[test]
    fn rejects_garbage_output() {
        assert_eq!(parse_llama_version(""), None);
        assert_eq!(parse_llama_version("llama-server: command not found"), None);
        assert_eq!(
            parse_llama_version("version: unknown (build x, commit y)"),
            None
        );
        assert_eq!(parse_llama_version("version:\r\n"), None);
        assert_eq!(
            parse_llama_version("built with Clang 20.1.8 for Windows x86_64\r\n"),
            None
        );
    }

    #[test]
    fn verified_is_none_without_expected_or_actual_digest() {
        let no_expected = model(Some(source_with_sha(None)));
        assert_eq!(
            build_manifest(input(&no_expected, Some(ABC_SHA256.to_owned()))).verified,
            None
        );
        let expected = model(Some(source_with_sha(Some(ABC_SHA256))));
        assert_eq!(build_manifest(input(&expected, None)).verified, None);
    }

    #[test]
    fn verified_compares_digests_case_insensitively() {
        let upper = ABC_SHA256.to_uppercase();
        let expected = model(Some(source_with_sha(Some(upper.as_str()))));
        let manifest = build_manifest(input(&expected, Some(ABC_SHA256.to_owned())));
        assert_eq!(manifest.verified, Some(true));
        assert_eq!(manifest.expected_sha256.as_deref(), Some(upper.as_str()));
    }

    #[test]
    fn verified_is_false_on_mismatch() {
        let expected = model(Some(source_with_sha(Some(
            "0000000000000000000000000000000000000000000000000000000000000000",
        ))));
        let manifest = build_manifest(input(&expected, Some(ABC_SHA256.to_owned())));
        assert_eq!(manifest.verified, Some(false));
    }

    #[test]
    fn build_manifest_copies_source_and_runtime_fields() {
        let config = model(Some(source_with_sha(None)));
        let runtime = RuntimeVersion {
            build: "11756".to_owned(),
            commit: Some("d80619397".to_owned()),
        };
        let manifest = build_manifest(ManifestInput {
            runtime: Some(runtime),
            ..input(&config, None)
        });
        assert_eq!(manifest.model, "qwen-main");
        assert_eq!(manifest.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(manifest.source.as_deref(), Some("unsloth/Qwen3.5-9B-GGUF"));
        assert_eq!(manifest.revision.as_deref(), Some("abc123"));
        assert_eq!(manifest.license.as_deref(), Some("apache-2.0"));
        assert_eq!(manifest.runtime_build.as_deref(), Some("11756"));
        assert_eq!(manifest.runtime_commit.as_deref(), Some("d80619397"));
        assert!(manifest.created_at.ends_with('Z'));
    }

    #[test]
    fn manifest_json_round_trips_through_write_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = model(Some(source_with_sha(Some(ABC_SHA256))));
        let manifest = build_manifest(input(&config, Some(ABC_SHA256.to_owned())));
        let path = write_manifest(&dir.path().join("manifests"), &manifest).expect("write");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("main.json"));
        let back: ModelManifest =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        assert_eq!(back, manifest);
    }

    #[test]
    fn write_manifest_rejects_slot_names_that_are_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = model(None);
        let mut manifest = build_manifest(input(&config, None));
        manifest.slot = "../escape".to_owned();
        let err = write_manifest(dir.path(), &manifest).expect_err("path separator rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
