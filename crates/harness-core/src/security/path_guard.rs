//! Workspace path guard (R-27.1): every file path a tool touches must resolve inside an
//! authorised root.
//!
//! IMPLEMENTATION SPEC (delegated):
//!
//! `PathGuard::new(roots, windows_rules)` canonicalises each root with `dunce::canonicalize`
//! (roots must exist; return the io error otherwise) and stores them.
//!
//! `PathGuard::check(candidate, base)` → `Ok(resolved)` or `Err(PathViolation)`, in this order:
//! 1. Empty candidate → `Empty`. Candidate containing a NUL byte → `InvalidCharacter`.
//! 2. If `windows_rules`:
//!    - UNC / device-namespace prefixes (`\\server\share`, `//server/share`, `\\?\`, `\\.\`) →
//!      `UncOrDevicePath` (checked on the raw string, both slash styles).
//!    - Any component whose name before the first `.` (case-insensitive, trailing spaces/dots
//!      trimmed) is a reserved device name (CON, PRN, AUX, NUL, COM1–COM9, LPT1–LPT9, also the
//!      superscript variants COM¹ COM² COM³ LPT¹ LPT² LPT³) → `ReservedDeviceName(component)`.
//!    - A `:` anywhere except as the drive-letter colon at index 1 → `AlternateDataStream`.
//!    - A component ending in `.` or ` ` (Windows silently strips these) → `TrailingDotOrSpace`.
//! 3. Join: absolute candidates (use `config::is_absolute_any_platform` semantics on the string,
//!    but join with std `Path` semantics) are used as-is; relative ones are joined onto `base`.
//! 4. Lexically normalise (drop `.`, pop on `..`; popping above the root → `Traversal`).
//! 5. Lexical containment: the normalised path must start with some root (component-wise,
//!    case-insensitive when `windows_rules`) → otherwise `OutsideWorkspace { resolved }`.
//! 6. Physical containment: find the longest existing ancestor of the normalised path,
//!    `dunce::canonicalize` it (this resolves symlinks and junctions), re-append the remaining
//!    non-existent components, and check containment again. A path that was lexically inside but
//!    physically outside → `SymlinkEscape { resolved }`.
//! 7. Return the physically resolved path.
//!
//! `windows_rules` defaults to `cfg!(windows)` via `PathGuard::for_platform`, but is a parameter
//! so Linux CI exercises the Windows rules too.
//!
//! Tests live in `crates/harness-core/tests/security_paths.rs`: `../` traversal, absolute escape
//! (`/etc/passwd`, `C:\Windows\...` with windows_rules), symlink escape (unix only, `#[cfg(unix)]`,
//! using `std::os::unix::fs::symlink` in a tempdir pointing outside), symlink *inside* the root is
//! allowed, non-existent files inside root are allowed (new files), every device-name form, ADS
//! (`a.txt:stream`, `a.txt::$DATA`), UNC/`\\?\`, trailing dot/space, multiple roots, case-insensitive
//! containment under windows_rules, and `..` that stays inside (`src/../README.md`) is allowed.

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PathViolation {
    #[error("empty path")]
    Empty,
    #[error("path contains an invalid character")]
    InvalidCharacter,
    #[error("path traverses above the filesystem root")]
    Traversal,
    #[error("path resolves outside the workspace: {resolved}")]
    OutsideWorkspace { resolved: String },
    #[error("path escapes the workspace through a symlink or junction: {resolved}")]
    SymlinkEscape { resolved: String },
    #[error("reserved Windows device name: {0}")]
    ReservedDeviceName(String),
    #[error("alternate data stream syntax is not allowed")]
    AlternateDataStream,
    #[error("UNC and device-namespace paths are not allowed")]
    UncOrDevicePath,
    #[error("path component ends with a dot or space")]
    TrailingDotOrSpace,
}

#[derive(Debug, Clone)]
pub struct PathGuard {
    roots: Vec<PathBuf>,
    windows_rules: bool,
}

impl PathGuard {
    pub fn new(
        roots: impl IntoIterator<Item = PathBuf>,
        windows_rules: bool,
    ) -> std::io::Result<Self> {
        let _ = (roots.into_iter().count(), windows_rules);
        todo!("canonicalise roots")
    }

    /// Guard with platform-default rules (`windows_rules = cfg!(windows)`).
    pub fn for_platform(roots: impl IntoIterator<Item = PathBuf>) -> std::io::Result<Self> {
        Self::new(roots, cfg!(windows))
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn windows_rules(&self) -> bool {
        self.windows_rules
    }

    pub fn check(&self, candidate: &Path, base: &Path) -> Result<PathBuf, PathViolation> {
        let _ = (candidate, base);
        todo!("see IMPLEMENTATION SPEC")
    }
}
