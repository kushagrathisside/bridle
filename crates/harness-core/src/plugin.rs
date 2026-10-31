//! Plugins (ADR-0013): a manifest declares tools, and each tool runs a fixed program with fixed arguments.
//!
//! The class a tool is offered under is the stricter of two things: the class its manifest declares, and the class the
//! command classifier gives the command it runs. A plugin therefore cannot make a tool more permissive than the classifier
//! says, and a tool the classifier calls risky is not offered at all. The manifest is only read from the machine config
//! (`[plugins]`); a repository cannot add tools.
//!
//! This module is the pure part: the manifest format, its checks, and the classification rule. The loader and the tool
//! wiring are in `harness-runtime::plugin`.

use serde::Deserialize;

use crate::security::ToolClass;
use crate::security::command_risk::{ShellKind, assess_command};

/// The largest manifest read, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// The most tools one manifest may declare.
pub const MAX_TOOLS: usize = 16;
/// The longest description a tool may have, in characters.
pub const MAX_DESCRIPTION: usize = 200;

/// The class a manifest may declare for a tool. A plugin can only ask for the two classes below; anything else is decided
/// by the classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeclaredClass {
    ReadOnly,
    ExecuteSafe,
}

/// One tool a manifest declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDecl {
    pub name: String,
    pub description: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub declared: DeclaredClass,
}

/// A plugin manifest: a name, a version, and the tools it declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<ToolDecl>,
}

/// Parses a manifest and checks it. A manifest that fails a check is not loaded.
pub fn parse(text: &str) -> Result<Manifest, String> {
    if text.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "the manifest is longer than {MAX_MANIFEST_BYTES} bytes"
        ));
    }
    let manifest: Manifest =
        toml::from_str(text).map_err(|error| format!("the manifest is not valid TOML: {error}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

/// The checks a manifest must pass: identifiers are short and plain; the program is a bare name; the arguments carry no
/// shell syntax; there is at least one tool and at most [`MAX_TOOLS`]; tool names are unique.
pub fn validate(manifest: &Manifest) -> Result<(), String> {
    if !is_identifier(&manifest.name, 32) {
        return Err(format!(
            "the plugin name {:?} must be 1 to 32 characters of a-z, 0-9, _ or -",
            manifest.name
        ));
    }
    if manifest.version.trim().is_empty() || manifest.version.len() > 32 {
        return Err("the plugin version must be 1 to 32 characters".to_owned());
    }
    if manifest.tools.is_empty() || manifest.tools.len() > MAX_TOOLS {
        return Err(format!(
            "a plugin declares 1 to {MAX_TOOLS} tools; this one declares {}",
            manifest.tools.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for tool in &manifest.tools {
        if !is_identifier(&tool.name, 32) {
            return Err(format!(
                "the tool name {:?} must be 1 to 32 characters of a-z, 0-9, _ or -",
                tool.name
            ));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err(format!("the tool name {:?} is declared twice", tool.name));
        }
        let description = tool.description.trim();
        if description.is_empty() || description.chars().count() > MAX_DESCRIPTION {
            return Err(format!(
                "the tool {:?} needs a description of 1 to {MAX_DESCRIPTION} characters",
                tool.name
            ));
        }
        if tool.program.is_empty()
            || tool.program.contains(['/', '\\', ' ', '\t'])
            || tool.program.chars().any(char::is_control)
        {
            return Err(format!(
                "the program of {:?} must be a bare name, with no path and no spaces",
                tool.name
            ));
        }
        if let Some(bad) = tool.args.iter().find(|arg| !plain_argument(arg)) {
            return Err(format!(
                "an argument of {:?} uses shell syntax or control characters: {bad:?}",
                tool.name
            ));
        }
    }
    Ok(())
}

/// The class the command classifier gives a tool's command, read as POSIX words. Commands run without a shell, so the
/// words are the command.
pub fn assessed_class(tool: &ToolDecl) -> ToolClass {
    let mut line = tool.program.clone();
    for arg in &tool.args {
        line.push(' ');
        line.push_str(arg);
    }
    assess_command(&line, ShellKind::Posix, &[]).class
}

/// The class a tool is offered under: the stricter of its declared class and the classifier's class.
pub fn effective_class(declared: DeclaredClass, assessed: ToolClass) -> ToolClass {
    let declared = match declared {
        DeclaredClass::ReadOnly => ToolClass::ReadOnly,
        DeclaredClass::ExecuteSafe => ToolClass::ExecuteSafe,
    };
    if rank(assessed) >= rank(declared) {
        assessed
    } else {
        declared
    }
}

/// How permissive a class is: lower is more permissive. Anything above execution is treated as the most restrictive.
fn rank(class: ToolClass) -> u8 {
    match class {
        ToolClass::ReadOnly => 0,
        ToolClass::ExecuteSafe => 1,
        _ => 2,
    }
}

fn is_identifier(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.chars().count() <= max
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn plain_argument(arg: &str) -> bool {
    !arg.is_empty()
        && arg.len() <= 256
        && !arg
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || ";&|<>$`\"'\\(){}*?[]!#~".contains(c))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const VALID: &str = r#"
name = "lint-tools"
version = "0.1.0"

[[tools]]
name = "version"
description = "List the repository root"
program = "ls"
args = ["-1"]
declared = "READ_ONLY"
"#;

    #[test]
    fn a_valid_manifest_is_read() {
        let manifest = parse(VALID).unwrap();
        assert_eq!(manifest.name, "lint-tools");
        assert_eq!(manifest.tools[0].declared, DeclaredClass::ReadOnly);
    }

    #[test]
    fn a_manifest_outside_the_checks_is_refused() {
        let bad_name = VALID.replace("lint-tools", "Lint Tools");
        assert!(parse(&bad_name).unwrap_err().contains("plugin name"));
        let path = VALID.replace("program = \"ls\"", "program = \"/usr/bin/ls\"");
        assert!(parse(&path).unwrap_err().contains("bare name"));
        let shell = VALID.replace("-1\"]", "-1;rm\"]");
        assert!(parse(&shell).unwrap_err().contains("shell syntax"));
        let writable = VALID.replace("READ_ONLY", "WRITE");
        assert!(
            parse(&writable).is_err(),
            "only READ_ONLY and EXECUTE_SAFE may be declared"
        );
        let unknown = format!("{VALID}\nextra = 1\n");
        assert!(parse(&unknown).is_err(), "unknown keys are refused");
        let none = "name = \"p\"\nversion = \"1\"\ntools = []\n";
        assert!(parse(none).unwrap_err().contains("1 to 16 tools"));
    }

    #[test]
    fn duplicate_tool_names_are_refused() {
        let doubled = format!(
            "{VALID}\n[[tools]]\nname = \"version\"\ndescription = \"again\"\nprogram = \"true\"\ndeclared = \"EXECUTE_SAFE\"\n"
        );
        assert!(parse(&doubled).unwrap_err().contains("declared twice"));
    }

    #[test]
    fn a_tool_is_offered_under_the_stricter_of_declared_and_assessed() {
        // A plugin cannot ask for more than the classifier allows.
        assert_eq!(
            effective_class(DeclaredClass::ReadOnly, ToolClass::ReadOnly),
            ToolClass::ReadOnly
        );
        assert_eq!(
            effective_class(DeclaredClass::ReadOnly, ToolClass::ExecuteSafe),
            ToolClass::ExecuteSafe
        );
        assert_eq!(
            effective_class(DeclaredClass::ReadOnly, ToolClass::ExecuteRisky),
            ToolClass::ExecuteRisky
        );
        // A plugin cannot ask for less than it declares either.
        assert_eq!(
            effective_class(DeclaredClass::ExecuteSafe, ToolClass::ReadOnly),
            ToolClass::ExecuteSafe
        );
        assert_eq!(
            effective_class(DeclaredClass::ExecuteSafe, ToolClass::Network),
            ToolClass::Network
        );
    }

    #[test]
    fn the_classifier_judges_the_declared_command() {
        let manifest = parse(VALID).unwrap();
        let read = assessed_class(&manifest.tools[0]);
        assert!(
            matches!(read, ToolClass::ReadOnly | ToolClass::ExecuteSafe),
            "{read:?}"
        );
        let risky = ToolDecl {
            name: "wipe".to_owned(),
            description: "remove files".to_owned(),
            program: "rm".to_owned(),
            args: vec!["-rf".to_owned(), "build".to_owned()],
            declared: DeclaredClass::ExecuteSafe,
        };
        assert!(!matches!(
            assessed_class(&risky),
            ToolClass::ReadOnly | ToolClass::ExecuteSafe
        ));
    }
}
