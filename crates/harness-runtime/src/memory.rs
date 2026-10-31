//! Repository memory, read: the file `[memory]` names, checked by the tool policy, redacted, and framed for the
//! system prompt. The pure part (the path rule and the framing) is `harness_core::memory`.
//!
//! A repository file is read like any other file the model could ask for, so the policy applies: a secret file
//! named as memory is refused, and a symlink out of the workspace is refused. The file is redacted before it is
//! framed, so a secret value in it never reaches the prompt.

use std::io::Read;
use std::path::Path;

use harness_core::config::{MemorySection, Verdict};
use harness_core::memory;
use harness_core::security::redact::Redactor;
use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};

/// Bytes read past the limit, so a secret that straddles the cut is redacted whole before the cut.
const READ_MARGIN: usize = 4_096;

/// The memory block for the system prompt. `Ok(None)` when memory is off, the file is missing, or it is empty.
/// `Err` when the file name is not allowed or the policy refuses the file, so the caller can say why nothing was
/// added.
pub fn load(
    config: &MemorySection,
    workspace: &Path,
    policy: &ToolPolicy,
    redactor: &Redactor,
) -> Result<Option<String>, String> {
    if !config.enabled {
        return Ok(None);
    }
    memory::check_file(&config.file).map_err(|reason| format!("memory.file {reason}"))?;
    if !workspace.join(&config.file).is_file() {
        return Ok(None);
    }
    let mut tool = ToolRequest::new(ToolKind::Read);
    tool.paths = vec![config.file.clone()];
    let decision = policy.evaluate(&tool);
    if decision.verdict != Verdict::Allow {
        return Err(format!(
            "{} was refused by policy ({}): {}",
            config.file,
            decision.rule,
            decision.reasons.join("; ")
        ));
    }
    let file = std::fs::File::open(workspace.join(&config.file))
        .map_err(|error| format!("cannot read {}: {error}", config.file))?;
    let mut bytes = Vec::new();
    file.take((config.max_bytes + READ_MARGIN) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", config.file))?;
    let lossy = String::from_utf8_lossy(&bytes);
    let text = redactor.redact(&lossy);
    Ok(memory::frame(&text, config.max_bytes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::SecuritySection;

    fn policy(dir: &Path) -> ToolPolicy {
        ToolPolicy::with_rules(dir, &SecuritySection::default(), false).unwrap()
    }

    fn section(file: &str) -> MemorySection {
        MemorySection {
            file: file.to_owned(),
            ..MemorySection::default()
        }
    }

    #[test]
    fn a_missing_file_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let got = load(
            &section("AGENTS.md"),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
        );
        assert_eq!(got, Ok(None));
    }

    #[test]
    fn a_disabled_memory_reads_nothing_even_if_the_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Use tabs.").unwrap();
        let mut config = section("AGENTS.md");
        config.enabled = false;
        assert_eq!(
            load(&config, dir.path(), &policy(dir.path()), &Redactor::new()),
            Ok(None)
        );
    }

    #[test]
    fn the_file_is_framed_and_its_secret_values_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "Run the tests before you finish.\napi_key = supersecret123\n",
        )
        .unwrap();
        let block = load(
            &section("AGENTS.md"),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
        )
        .unwrap()
        .unwrap();
        assert!(block.starts_with(memory::HEADER));
        assert!(block.contains("Run the tests before you finish."));
        assert!(!block.contains("supersecret123"));
    }

    #[test]
    fn a_secret_file_named_as_memory_is_refused_by_the_policy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "TOKEN=abc\n").unwrap();
        let got = load(
            &section(".env"),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
        );
        let error = got.unwrap_err();
        assert!(error.contains(".env was refused by policy"), "{error}");
        assert!(!error.contains("abc"));
    }

    #[test]
    fn a_name_that_leaves_the_repository_is_refused_before_any_read() {
        let dir = tempfile::tempdir().unwrap();
        let got = load(
            &section("../AGENTS.md"),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
        );
        assert!(got.unwrap_err().contains("must stay inside the repository"));
    }

    #[test]
    fn a_long_file_is_cut_to_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "x".repeat(20_000)).unwrap();
        let mut config = section("AGENTS.md");
        config.max_bytes = 512;
        let block = load(&config, dir.path(), &policy(dir.path()), &Redactor::new())
            .unwrap()
            .unwrap();
        assert!(block.ends_with("[shortened to 512 bytes]"));
        assert!(block.len() < 512 + memory::HEADER.len() + 64);
    }
}
