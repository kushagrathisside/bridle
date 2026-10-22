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
//!    - Any candidate (either platform) whose meaning depends on expansion (`~`, `$VAR`, `${VAR}`,
//!      `$(...)`, backticks, `%NAME%`) → `Unresolved`: the guard cannot know what it names, so it
//!      must not join it under `base`. Checked after the syntax rules above.
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

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::config::is_absolute_any_platform;

/// Why a candidate path was refused. Serialised with a `kind` tag.
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
    #[error(
        "path uses a shell or host expansion (~, $, backtick or %NAME%) that cannot be resolved"
    )]
    Unresolved,
}

/// Confines file operations to a fixed set of canonical roots. The check order is documented in
/// the module-level spec.
#[derive(Debug, Clone)]
pub struct PathGuard {
    roots: Vec<PathBuf>,
    windows_rules: bool,
}

impl PathGuard {
    /// Canonicalises each root with `dunce::canonicalize` and stores it. Every root must exist.
    ///
    /// # Errors
    /// Returns the I/O error of the first root that cannot be canonicalised.
    pub fn new(
        roots: impl IntoIterator<Item = PathBuf>,
        windows_rules: bool,
    ) -> std::io::Result<Self> {
        let roots = roots
            .into_iter()
            .map(dunce::canonicalize)
            .collect::<std::io::Result<Vec<PathBuf>>>()?;
        Ok(Self {
            roots,
            windows_rules,
        })
    }

    /// Guard with platform-default rules (`windows_rules = cfg!(windows)`).
    pub fn for_platform(roots: impl IntoIterator<Item = PathBuf>) -> std::io::Result<Self> {
        Self::new(roots, cfg!(windows))
    }

    /// The canonical roots, in the order they were given.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether Windows naming rules are applied.
    pub fn windows_rules(&self) -> bool {
        self.windows_rules
    }

    /// Resolves `candidate` (relative candidates are joined onto `base`) and returns the physically
    /// resolved path if it lies inside one of the roots.
    ///
    /// Callers must operate on the returned path, not on `candidate`. `base` should be spelled the
    /// way the roots are (canonical), because containment is compared lexically before it is
    /// compared physically.
    ///
    /// # Errors
    /// Returns the first [`PathViolation`] found, following the check order in the module spec.
    pub fn check(&self, candidate: &Path, base: &Path) -> Result<PathBuf, PathViolation> {
        if candidate.as_os_str().is_empty() {
            return Err(PathViolation::Empty);
        }
        if has_nul(candidate) || has_nul(base) {
            return Err(PathViolation::InvalidCharacter);
        }
        let candidate = if self.windows_rules {
            let raw = candidate.to_str().ok_or(PathViolation::InvalidCharacter)?;
            check_windows_syntax(raw)?;
            // Windows treats `\` and `/` alike; Linux `Path` only splits on `/`, so unify first.
            PathBuf::from(raw.replace('\\', "/"))
        } else {
            candidate.to_path_buf()
        };
        // After the Windows syntax check, so that `a.txt::$DATA` still reports an alternate stream.
        let text = candidate.to_string_lossy();
        if is_unresolvable(&text) || has_percent_expansion(&text) {
            return Err(PathViolation::Unresolved);
        }

        // Under windows_rules a leading separator without a drive is root-relative on Windows, so
        // it must not be joined onto `base` on any host.
        let rooted =
            self.windows_rules && matches!(candidate.as_os_str().as_encoded_bytes(), [b'/', ..]);
        let joined = if rooted || is_absolute_any_platform(&candidate) {
            candidate
        } else {
            base.join(&candidate)
        };

        let lexical = lexical_normalise(&joined)?;
        let contained_lexically = self
            .roots
            .iter()
            .any(|root| rebase(&lexical, root, self.windows_rules).is_some());
        if !contained_lexically {
            return Err(PathViolation::OutsideWorkspace {
                resolved: display(&lexical),
            });
        }

        let physical = physical_path(&lexical)?;
        self.roots
            .iter()
            .find_map(|root| rebase(&physical, root, self.windows_rules))
            .ok_or_else(|| PathViolation::SymlinkEscape {
                resolved: display(&physical),
            })
    }
}

/// Windows-only checks on the raw text of a candidate (step 2). They run on the string because
/// `Path` on Linux does not split on `\`.
fn check_windows_syntax(raw: &str) -> Result<(), PathViolation> {
    if matches!(raw.as_bytes(), [a, b, ..] if is_separator_byte(*a) && is_separator_byte(*b)) {
        return Err(PathViolation::UncOrDevicePath);
    }

    let names: Vec<&str> = raw
        .split(is_separator)
        .filter(|name| !name.is_empty())
        .collect();
    if let Some(name) = names
        .iter()
        .copied()
        .find(|name| is_reserved_device_name(name))
    {
        return Err(PathViolation::ReservedDeviceName(name.to_owned()));
    }

    if !colons_only_in_drive_prefix(raw) {
        return Err(PathViolation::AlternateDataStream);
    }

    if names.iter().any(|name| ends_with_dot_or_space(name)) {
        return Err(PathViolation::TrailingDotOrSpace);
    }
    Ok(())
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

fn is_separator_byte(b: u8) -> bool {
    b == b'\\' || b == b'/'
}

/// Reserved DOS device names, matched on the text before the first dot, case-insensitively,
/// ignoring surrounding spaces. `COM0`/`LPT0` and `CONIN$`/`CONOUT$` are refused too, as a
/// conservative superset.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim_matches(' ')
        .to_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        other => other
            .strip_prefix("COM")
            .or_else(|| other.strip_prefix("LPT"))
            .is_some_and(|n| {
                matches!(
                    n,
                    "0" | "1"
                        | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
            }),
    }
}

/// A `:` is legal only as the drive colon of `X:\` or `X:/`. Anything else is refused, including
/// `name:stream` (alternate data streams) and `C:foo` (drive-relative).
fn colons_only_in_drive_prefix(raw: &str) -> bool {
    let colons = raw.matches(':').count();
    let drive = matches!(
        raw.as_bytes(),
        [letter, b':', sep, ..] if letter.is_ascii_alphabetic() && is_separator_byte(*sep)
    );
    colons == 0 || (colons == 1 && drive)
}

fn ends_with_dot_or_space(name: &str) -> bool {
    // `.` and `..` are navigation components, not names, so they are not stripped by Windows.
    !matches!(name, "." | "..") && name.ends_with(['.', ' '])
}

/// Step 4: removes `.` and resolves `..` without touching the filesystem. A `..` with nothing
/// left to pop (above the root, or at the start of a relative path) is `Traversal`.
fn lexical_normalise(path: &Path) -> Result<PathBuf, PathViolation> {
    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(PathViolation::Traversal);
                }
                out.pop();
                depth -= 1;
            }
            Component::Normal(name) => {
                out.push(name);
                depth += 1;
            }
        }
    }
    Ok(out)
}

/// Step 6: canonicalises the longest existing ancestor of `lexical` (following symlinks and
/// junctions) and re-appends the components that do not exist yet.
///
/// "Exists" means `symlink_metadata` succeeds, so a dangling symlink counts as existing. An
/// existing ancestor that cannot be canonicalised (dangling, a symlink loop, or unreadable) is
/// refused as `SymlinkEscape`: it cannot be shown to stay inside the root.
fn physical_path(lexical: &Path) -> Result<PathBuf, PathViolation> {
    let escape = || PathViolation::SymlinkEscape {
        resolved: display(lexical),
    };
    let mut ancestor = lexical;
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(_) => return Err(escape()),
        }
        let Some(parent) = ancestor.parent() else {
            return Err(PathViolation::OutsideWorkspace {
                resolved: display(lexical),
            });
        };
        ancestor = parent;
    }

    let mut physical = dunce::canonicalize(ancestor).map_err(|_| escape())?;
    let rest = lexical.strip_prefix(ancestor).map_err(|_| escape())?;
    for component in rest.components() {
        physical.push(component.as_os_str());
    }
    Ok(physical)
}

/// If `root` is a component-wise prefix of `path`, returns `path` re-expressed under `root`'s
/// spelling; otherwise `None`. With `case_insensitive` the components are compared ignoring case.
fn rebase(path: &Path, root: &Path, case_insensitive: bool) -> Option<PathBuf> {
    let mut remaining = path.components();
    for root_component in root.components() {
        if !component_eq(remaining.next()?, root_component, case_insensitive) {
            return None;
        }
    }
    let mut out = root.to_path_buf();
    for component in remaining {
        out.push(component.as_os_str());
    }
    Some(out)
}

fn component_eq(a: Component<'_>, b: Component<'_>, case_insensitive: bool) -> bool {
    let (a, b) = (a.as_os_str(), b.as_os_str());
    if a == b {
        return true;
    }
    // Non-UTF-8 names are never folded: a lossy comparison could merge distinct names.
    match (case_insensitive, a.to_str(), b.to_str()) {
        (true, Some(x), Some(y)) => x.to_lowercase() == y.to_lowercase(),
        _ => false,
    }
}

fn has_nul(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().contains(&0)
}

/// Whether `word` starts with a tilde prefix (`~`, `~/x`, `~user`, `~+`, `~-`), which the shell
/// expands to a home directory, or contains a `$` expansion or a backtick substitution. Such a word
/// names a location only at run time. A `~` followed by other text (`~%h`) is not a tilde prefix.
pub fn is_unresolvable(word: &str) -> bool {
    starts_with_tilde_prefix(word) || has_dollar_expansion(word) || word.contains('`')
}

fn starts_with_tilde_prefix(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('~') else {
        return false;
    };
    rest.is_empty()
        || rest.starts_with(['/', '\\', '+', '-'])
        || rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
}

/// A `$` followed by a character that starts a parameter, command, arithmetic or ANSI-C
/// expansion. A `$` at the end of a word (`'a$'`, a regex anchor) is literal.
fn has_dollar_expansion(word: &str) -> bool {
    word.chars().zip(word.chars().skip(1)).any(|(c, next)| {
        c == '$' && (next.is_ascii_alphanumeric() || "_{([!#?*@$-'\"".contains(next))
    })
}

/// A Windows-style `%NAME%` variable expansion (cmd, batch files). A name is the text between two
/// `%` signs, without whitespace or path separators. Shell words such as the `%Y%m%d` format of
/// `date` are not paths, so callers apply this test only to path-like words.
pub fn has_percent_expansion(word: &str) -> bool {
    let mut rest = word;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            return false;
        };
        let name = &after[..end];
        if !name.is_empty()
            && !name
                .chars()
                .any(|c| c.is_whitespace() || "/\\%".contains(c))
        {
            return true;
        }
        // The closing `%` may open the next pair, so scanning resumes there.
        rest = &after[end..];
    }
    false
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_match_before_the_first_dot_case_insensitively() {
        for name in [
            "CON",
            "prn",
            "Aux.c",
            "nul.tar.gz",
            "COM1",
            "lpt9",
            "COM0",
            "com\u{b9}",
            "LPT\u{b3}.log",
            "CON ",
            "NUL.",
            "CONIN$",
        ] {
            assert!(is_reserved_device_name(name), "{name} should be reserved");
        }
        for name in [
            "CONSOLE",
            "COM10",
            "auxiliary.txt",
            "nul_notes",
            "src",
            "C:",
        ] {
            assert!(
                !is_reserved_device_name(name),
                "{name} should not be reserved"
            );
        }
    }

    #[test]
    fn lexical_normalise_drops_dots_and_pops_parents() {
        let out = lexical_normalise(Path::new("/a/./b/../c")).expect("normal path");
        assert_eq!(out, Path::new("/a/c"));
    }

    #[test]
    fn lexical_normalise_refuses_to_pop_past_the_root() {
        assert_eq!(
            lexical_normalise(Path::new("/a/../../b")),
            Err(PathViolation::Traversal)
        );
        assert_eq!(
            lexical_normalise(Path::new("../b")),
            Err(PathViolation::Traversal)
        );
    }

    #[test]
    fn rebase_matches_whole_components_only() {
        assert!(rebase(Path::new("/tmp/ws2/x"), Path::new("/tmp/ws"), false).is_none());
        assert!(rebase(Path::new("/tmp/WS/x"), Path::new("/tmp/ws"), false).is_none());
        assert_eq!(
            rebase(Path::new("/tmp/WS/x"), Path::new("/tmp/ws"), true),
            Some(PathBuf::from("/tmp/ws/x"))
        );
    }

    /// Item 13: the guard never joins an expansion under the base; ordinary punctuation is fine.
    #[test]
    fn expansions_are_unresolved_and_ordinary_punctuation_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let guard = PathGuard::new(vec![dir.path().to_path_buf()], false).expect("guard");
        for p in [
            "~/.bashrc",
            "~",
            "~user/x",
            "$HOME/x",
            "${HOME}/x",
            "$(pwd)/x",
            "`pwd`/x",
            "%USERPROFILE%\\x",
            "a/%TEMP%/x",
        ] {
            assert_eq!(
                guard.check(Path::new(p), dir.path()),
                Err(PathViolation::Unresolved),
                "{p}"
            );
        }
        for p in ["src/main.rs", "notes~.txt", "cost$.txt", "100%.txt"] {
            assert!(guard.check(Path::new(p), dir.path()).is_ok(), "{p}");
        }
    }

    #[test]
    fn windows_syntax_checks_both_separator_styles() {
        assert_eq!(
            check_windows_syntax(r"\\server\share\f"),
            Err(PathViolation::UncOrDevicePath)
        );
        assert_eq!(
            check_windows_syntax("//server/share/f"),
            Err(PathViolation::UncOrDevicePath)
        );
        assert_eq!(
            check_windows_syntax(r"src\aux.c"),
            Err(PathViolation::ReservedDeviceName("aux.c".to_owned()))
        );
        assert_eq!(check_windows_syntax(r"C:\ok\file"), Ok(()));
    }
}
