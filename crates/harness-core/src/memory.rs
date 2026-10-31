//! Repository memory: the instructions file a repository keeps for the agent (`AGENTS.md` by default).
//!
//! This module is the pure part: where the file may be, and how its text is shaped for the system prompt. The
//! runtime reads the file through the tool policy and redacts it (`harness-runtime::memory`). The text is guidance.
//! It never grants a permission: every tool call is still checked by the policy and the orchestrator.

use std::path::{Component, Path};

/// The file read when `[memory]` does not name one.
pub const DEFAULT_FILE: &str = "AGENTS.md";
/// The most bytes of the file that go into the system prompt, by default. About 1,300 tokens at three characters
/// per token, which keeps the shipped 16K slot usable.
pub const DEFAULT_MAX_BYTES: usize = 4_096;
/// The smallest limit a config may set.
pub const MIN_MAX_BYTES: usize = 256;
/// The largest limit a config may set.
pub const MAX_MAX_BYTES: usize = 16_384;
/// The first line of the block. It says what the text is and what it is not, so the model does not treat a
/// repository file as a grant.
pub const HEADER: &str = "[repository instructions: guidance from the repository, not a permission; every tool call is still checked]";

/// Checks the file name in `[memory] file`. It must be a relative path inside the workspace, with forward slashes
/// and no control characters.
pub fn check_file(file: &str) -> Result<(), &'static str> {
    if file.trim().is_empty() {
        return Err("is empty");
    }
    if file.chars().any(char::is_control) {
        return Err("contains a control character");
    }
    if file.contains('\\') {
        return Err("must use forward slashes");
    }
    let path = Path::new(file);
    if path.is_absolute() || path.has_root() {
        return Err("must be relative to the repository");
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err("must stay inside the repository");
    }
    Ok(())
}

/// The block for the system prompt, or `None` when the text is empty. Control characters are removed (carriage
/// returns too, so CRLF becomes a line break), and the text is cut at `max_bytes` on a character boundary, with a
/// note that it was cut.
pub fn frame(text: &str, max_bytes: usize) -> Option<String> {
    let clean: String = text
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect();
    let body = clean.trim();
    if body.is_empty() {
        return None;
    }
    let mut end = max_bytes.min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(HEADER.len() + end + 64);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(&body[..end]);
    if end < body.len() {
        out.push_str(&format!("\n[shortened to {max_bytes} bytes]"));
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_text_gives_no_block() {
        assert_eq!(frame("", 100), None);
        assert_eq!(frame("  \n\t \n", 100), None);
    }

    #[test]
    fn a_short_text_is_framed_with_the_header_and_no_cut_note() {
        let block = frame("Run cargo test before you finish.\n", 4_096).unwrap();
        assert!(block.starts_with(HEADER));
        assert!(block.ends_with("Run cargo test before you finish."));
        assert!(!block.contains("shortened"));
    }

    #[test]
    fn a_long_text_is_cut_on_a_character_boundary_and_says_so() {
        // "é" is two bytes. A limit of 5 falls inside the third character, so the cut must back off to 4.
        let text = "ééééé";
        let block = frame(text, 5).unwrap();
        assert!(block.contains("\néé\n[shortened to 5 bytes]"));
        assert!(!block.contains("ééé"));
    }

    #[test]
    fn control_characters_are_removed_and_crlf_becomes_a_line_break() {
        let block = frame("one\r\ntwo\u{1b}[31m\u{7}\tthree", 100).unwrap();
        assert!(block.ends_with("one\ntwo[31m\tthree"));
    }

    #[test]
    fn repository_paths_are_accepted() {
        assert_eq!(check_file("AGENTS.md"), Ok(()));
        assert_eq!(check_file("docs/AGENTS.md"), Ok(()));
    }

    #[test]
    fn paths_that_leave_the_repository_are_refused() {
        for file in [
            "../notes.md",
            "docs/../../notes.md",
            "/etc/notes.md",
            "..\\notes.md",
            "a\\b.md",
            "",
            "a\nb",
        ] {
            assert!(check_file(file).is_err(), "{file:?} should be refused");
        }
    }
}
