//! Failure fingerprints (M3 step 3, R-19.1 to R-19.3).
//!
//! A fingerprint identifies one failure across attempts, so that the same failure seen three times
//! is recognised as a loop even when line numbers, addresses, timestamps or the counts in the message
//! change. The fields follow R-19.2: the failure kind, the tool, the command, the test identifier,
//! the source file without its line, the normalised message, and the exit code. The stack-trace
//! signature is not part of the parsed output, so it is not included; the format version says so.
//!
//! Fingerprints are computed on the unredacted text, in memory, before redaction (see the M3 plan).
//! The format is versioned with the `fp_v1:` prefix, so a later change to the normalisation is visible
//! in stored records and cannot silently make old hashes match new ones.

use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::failures::{Failure, FailureKind, FailureReport};

/// The prefix of every fingerprint produced by this version of the normalisation.
pub const FORMAT_PREFIX: &str = "fp_v1:";
/// Hex characters kept from the SHA-256 digest.
const HASH_HEX_CHARS: usize = 16;
/// Separates the fields before hashing. A unit separator cannot occur in parsed text.
const FIELD_SEPARATOR: char = '\u{1f}';

static ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").expect("valid regex"));
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\d{4}-\d{2}-\d{2}[t ]\d{2}:\d{2}:\d{2}(\.\d+)?(z|[+-]\d{2}:?\d{2})?")
        .expect("valid regex")
});
static WINDOWS_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-zA-Z]:[\\/][^\s'`]*").expect("valid regex"));
static UNIX_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|\s)/[\w.\-]+(/[\w.\-]+)+").expect("valid regex"));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[-+]?\d+").expect("valid regex"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));

/// The context a failure was seen in: the command that produced the output and its exit code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context<'a> {
    /// The command, as the caller normalised it (see `command_hash` in the loop detector).
    pub command: Option<&'a str>,
    /// The exit code of the command, when known.
    pub exit_code: Option<i32>,
}

/// The plain fields of one failure, as a fingerprint needs them. Built from a parsed [`Failure`], or
/// from the JSON of a stored result.
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    /// The failure kind name (see [`kind_name`]).
    pub kind: &'a str,
    pub tool: &'a str,
    pub name: Option<&'a str>,
    pub file: Option<&'a str>,
    pub message: &'a str,
}

/// The fingerprint of one failure in one context: `fp_v1:` and 16 hex characters.
pub fn fingerprint(failure: &Failure, context: Context<'_>) -> String {
    fingerprint_fields(
        &Fields {
            kind: kind_name(failure.kind),
            tool: failure.tool,
            name: failure.name.as_deref(),
            file: failure.file.as_deref(),
            message: &failure.message,
        },
        context,
    )
}

/// The fingerprint of a failure given as plain fields.
pub fn fingerprint_fields(fields: &Fields<'_>, context: Context<'_>) -> String {
    let parts = [
        format!("kind={}", fields.kind),
        format!("tool={}", fields.tool),
        format!(
            "command={}",
            context.command.map(collapse).unwrap_or_default()
        ),
        format!("test={}", fields.name.unwrap_or_default()),
        format!(
            "file={}",
            fields.file.map(normalise_file).unwrap_or_default()
        ),
        format!("message={}", normalise_message(fields.message)),
        format!(
            "exit={}",
            context
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_default()
        ),
    ];
    format!(
        "{FORMAT_PREFIX}{}",
        &digest_hex(&parts.join(&FIELD_SEPARATOR.to_string()))[..HASH_HEX_CHARS]
    )
}

/// A short hash of a shell command after whitespace normalisation. Two spellings of one command
/// hash alike; two different commands do not (the hash is taken before redaction, see the M3 plan).
pub fn command_hash(command: &str) -> String {
    digest_hex(&collapse(command))[..HASH_HEX_CHARS].to_owned()
}

/// A short hash of tool arguments. Object keys are sorted first, so the hash does not depend on key order.
pub fn args_hash(args: &serde_json::Value) -> String {
    digest_hex(&canonical_json(args))[..HASH_HEX_CHARS].to_owned()
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(key.clone()),
                        canonical_json(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        serde_json::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

fn digest_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(hex_digit)
        .collect()
}

/// The fingerprints of every failure in a report, in report order.
pub fn fingerprints(report: &FailureReport, context: Context<'_>) -> Vec<String> {
    report
        .failures
        .iter()
        .map(|failure| fingerprint(failure, context))
        .collect()
}

/// The message with the parts that change between attempts removed: hexadecimal addresses,
/// timestamps, absolute paths, and every run of digits (so `assert -1 == 3` and `assert 2 == 3`
/// are one failure). Case and whitespace are normalised.
pub fn normalise_message(message: &str) -> String {
    let lower = message.to_lowercase();
    let text = ADDRESS.replace_all(&lower, "<addr>");
    let text = TIMESTAMP.replace_all(&text, "<time>");
    let text = WINDOWS_PATH.replace_all(&text, "<path>");
    let text = UNIX_PATH.replace_all(&text, "$1<path>");
    let text = DIGITS.replace_all(&text, "#");
    collapse(&text)
}

fn normalise_file(file: &str) -> String {
    file.replace('\\', "/").to_lowercase()
}

fn collapse(text: &str) -> String {
    SPACES.replace_all(text.trim(), " ").into_owned()
}

/// The stable name of a failure kind, as stored in fingerprints.
pub fn kind_name(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::TestFailure => "test_failure",
        FailureKind::TestError => "test_error",
        FailureKind::CompileError => "compile_error",
        FailureKind::TypeError => "type_error",
        FailureKind::LintFinding => "lint_finding",
    }
}

fn hex_digit(nibble: u8) -> char {
    char::from(b"0123456789abcdef"[usize::from(nibble & 0x0f)])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::failures::parse_failures;

    fn failure(name: &str, file: &str, line: u32, message: &str) -> Failure {
        Failure {
            tool: "pytest",
            kind: FailureKind::TestFailure,
            name: Some(name.to_owned()),
            file: Some(file.to_owned()),
            line: Some(line),
            column: None,
            message: message.to_owned(),
        }
    }

    const NONE: Context<'static> = Context {
        command: None,
        exit_code: None,
    };

    #[test]
    fn the_format_is_versioned_with_sixteen_hex_characters() {
        let fp = fingerprint(
            &failure("test_add", "tests/test_calc.py", 5, "assert 1 == 2"),
            NONE,
        );
        let hex = fp.strip_prefix(FORMAT_PREFIX).expect("prefixed");
        assert_eq!(hex.len(), 16);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn moved_line_numbers_give_one_fingerprint() {
        let a = fingerprint(
            &failure("test_add", "tests/test_calc.py", 5, "assert -1 == 3"),
            NONE,
        );
        let b = fingerprint(
            &failure("test_add", "tests/test_calc.py", 41, "assert -1 == 3"),
            NONE,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn different_test_names_give_different_fingerprints() {
        let a = fingerprint(
            &failure("test_add", "tests/test_calc.py", 5, "assert 1 == 2"),
            NONE,
        );
        let b = fingerprint(
            &failure("test_sub", "tests/test_calc.py", 5, "assert 1 == 2"),
            NONE,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn different_files_give_different_fingerprints() {
        let a = fingerprint(
            &failure("test_add", "tests/test_calc.py", 5, "assert 1 == 2"),
            NONE,
        );
        let b = fingerprint(
            &failure("test_add", "tests/test_other.py", 5, "assert 1 == 2"),
            NONE,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn addresses_timestamps_and_paths_do_not_change_the_hash() {
        let a = failure(
            "t",
            "x.py",
            1,
            r"crash at 0x7ffd12ab34 on 2026-10-26 10:11:12 reading C:\Users\k\Temp\cache.bin",
        );
        let b = failure(
            "t",
            "x.py",
            1,
            "crash at 0x1 on 2027-01-02T03:04:05Z reading D:/other/place/cache.bin",
        );
        assert_eq!(fingerprint(&a, NONE), fingerprint(&b, NONE));
    }

    #[test]
    fn values_in_the_message_do_not_change_the_hash() {
        let a = fingerprint(&failure("t", "x.py", 1, "assert -1 == 3"), NONE);
        let b = fingerprint(&failure("t", "x.py", 1, "assert 2 == 3"), NONE);
        assert_eq!(a, b);
    }

    #[test]
    fn the_exit_code_and_the_command_are_part_of_the_hash() {
        let f = failure("t", "x.py", 1, "boom");
        let base = fingerprint(&f, NONE);
        let other_exit = fingerprint(
            &f,
            Context {
                command: None,
                exit_code: Some(2),
            },
        );
        let other_command = fingerprint(
            &f,
            Context {
                command: Some("python -m pytest -q"),
                exit_code: None,
            },
        );
        assert_ne!(base, other_exit);
        assert_ne!(base, other_command);
    }

    #[test]
    fn a_pytest_failure_parsed_twice_with_moved_lines_has_one_fingerprint() {
        let first = "\
tests/test_calc.py:5: in test_add
E       assert -1 == 3
FAILED tests/test_calc.py::test_add - assert -1 == 3
";
        let second = "\
============================= test session starts =============================
collected 1 item

tests/test_calc.py:19: in test_add
E       assert -1 == 3
FAILED tests/test_calc.py::test_add - assert -1 == 3
";
        let a = fingerprints(&parse_failures(first), NONE);
        let b = fingerprints(&parse_failures(second), NONE);
        assert!(!a.is_empty(), "the fixture must parse");
        assert_eq!(a, b);
    }

    #[test]
    fn command_hashes_ignore_spacing_and_differ_for_other_commands() {
        assert_eq!(
            command_hash("pytest  -q   x"),
            command_hash(" pytest -q x ")
        );
        assert_ne!(command_hash("pytest -q"), command_hash("pytest -x"));
        assert_eq!(command_hash("pytest").len(), 16);
    }

    #[test]
    fn argument_hashes_do_not_depend_on_key_order() {
        let a = serde_json::json!({"steps": ["unit"], "staged": false});
        let b: serde_json::Value =
            serde_json::from_str(r#"{"staged": false, "steps": ["unit"]}"#).unwrap();
        assert_eq!(args_hash(&a), args_hash(&b));
        assert_ne!(
            args_hash(&a),
            args_hash(&serde_json::json!({"steps": ["lint"]}))
        );
    }

    #[test]
    fn plain_fields_give_the_same_fingerprint_as_a_parsed_failure() {
        let f = failure("test_add", "tests/test_calc.py", 5, "assert -1 == 3");
        let from_fields = fingerprint_fields(
            &Fields {
                kind: kind_name(FailureKind::TestFailure),
                tool: "pytest",
                name: Some("test_add"),
                file: Some("tests/test_calc.py"),
                message: "assert -1 == 3",
            },
            NONE,
        );
        assert_eq!(fingerprint(&f, NONE), from_fields);
    }

    #[test]
    fn normalise_message_keeps_the_words_of_the_message() {
        assert_eq!(normalise_message("  Assert  FAILED  "), "assert failed");
    }
}
