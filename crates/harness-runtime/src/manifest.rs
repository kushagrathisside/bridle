//! Model manifests (§38): identity, revision, quantisation, hash, runtime build and hardware,
//! persisted so benchmark results are comparable (R-41).
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

use std::path::{Path, PathBuf};

use harness_core::config::ModelConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub build: String,
    pub commit: Option<String>,
}

/// Everything recorded about one model slot (§38).
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

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let _ = path;
    todo!()
}

pub fn cached_sha256(cache_path: &Path, file: &Path) -> std::io::Result<String> {
    let _ = (cache_path, file);
    todo!()
}

pub fn parse_llama_version(stdout: &str) -> Option<RuntimeVersion> {
    let _ = stdout;
    todo!()
}

pub fn build_manifest(input: ManifestInput<'_>) -> ModelManifest {
    let _ = input;
    todo!()
}

pub fn write_manifest(dir: &Path, manifest: &ModelManifest) -> std::io::Result<PathBuf> {
    let _ = (dir, manifest);
    todo!()
}
