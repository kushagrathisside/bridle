//! Plugin tools (ADR-0013): reads the manifests the machine config names, checks them, works out each tool's class, and
//! offers the tools whose class is not risky. A plugin tool runs its fixed program through the same checks as
//! `run_command`: the command is policy-checked, and it runs in the sandbox. A tool takes no arguments from the model.
//!
//! Each tool is named `plugin_<plugin>_<tool>`. The policy learns the class of each offered tool from [`Loaded::classes`],
//! and a plugin tool that is not in that table is an unknown tool and is blocked.

use std::collections::BTreeMap;
use std::path::Path;

use harness_core::config::Config;
use harness_core::plugin::{self, Manifest};
use harness_core::security::ToolClass;
use serde_json::{Value, json};

use super::run::execute_plain;
use super::{McpTool, ToolContext, ToolOutput};

/// What the loader found: the tools it offers, the class of each, and a report line for each manifest or tool it did not
/// load.
pub struct Loaded {
    pub tools: Vec<Box<dyn McpTool>>,
    pub classes: BTreeMap<String, ToolClass>,
    pub reports: Vec<String>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.tools.iter().map(|tool| tool.name()).collect();
        f.debug_struct("Loaded")
            .field("tools", &names)
            .field("reports", &self.reports)
            .finish()
    }
}

/// One plugin tool: a fixed program with fixed arguments.
#[derive(Debug)]
pub struct PluginTool {
    name: &'static str,
    description: &'static str,
    program: String,
    args: Vec<String>,
}

impl McpTool for PluginTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn call(&self, _args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let timeout = ctx.verification.default_timeout_ms.clamp(1_000, 600_000);
        execute_plain(ctx, &self.program, &self.args, None, timeout)
    }
}

/// Loads every manifest the configuration names. A manifest that cannot be read or checked is reported and skipped; so is a
/// tool whose class is risky or whose name is already taken.
pub fn load(config: &Config) -> Loaded {
    let mut loaded = Loaded {
        tools: Vec::new(),
        classes: BTreeMap::new(),
        reports: Vec::new(),
    };
    for path in &config.plugins.manifests {
        match read_manifest(Path::new(path)) {
            Ok(manifest) => offer(&manifest, &mut loaded),
            Err(reason) => loaded.reports.push(format!("{path}: not loaded: {reason}")),
        }
    }
    loaded
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > plugin::MAX_MANIFEST_BYTES as u64 {
        return Err(format!("longer than {} bytes", plugin::MAX_MANIFEST_BYTES));
    }
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    plugin::parse(&text)
}

fn offer(manifest: &Manifest, loaded: &mut Loaded) {
    for tool in &manifest.tools {
        let name = format!("plugin_{}_{}", manifest.name, tool.name);
        let class = plugin::effective_class(tool.declared, plugin::assessed_class(tool));
        // Only read-only and safe-execution tools are offered. Writes, network, destructive and risky commands are not.
        if !matches!(class, ToolClass::ReadOnly | ToolClass::ExecuteSafe) {
            loaded
                .reports
                .push(format!("{name}: not offered: its command is {class:?}"));
            continue;
        }
        if loaded.classes.contains_key(&name) {
            loaded
                .reports
                .push(format!("{name}: not offered: the name is already taken"));
            continue;
        }
        loaded.classes.insert(name.clone(), class);
        loaded.tools.push(Box::new(PluginTool {
            name: Box::leak(name.into_boxed_str()),
            description: Box::leak(tool.description.clone().into_boxed_str()),
            program: tool.program.clone(),
            args: tool.args.clone(),
        }));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn config_with(path: &Path) -> Config {
        let mut config = Config::default();
        config.plugins.manifests = vec![path.display().to_string()];
        config
    }

    #[test]
    fn a_read_only_tool_is_offered_under_its_name_and_class() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("plugin.toml");
        std::fs::write(
            &manifest,
            "name = \"files\"\nversion = \"0.1.0\"\n[[tools]]\nname = \"list\"\ndescription = \"List the root\"\nprogram = \"ls\"\nargs = [\"-1\"]\ndeclared = \"READ_ONLY\"\n",
        )
        .unwrap();
        let loaded = load(&config_with(&manifest));
        assert_eq!(loaded.tools.len(), 1, "{:?}", loaded.reports);
        assert_eq!(loaded.tools[0].name(), "plugin_files_list");
        assert!(matches!(
            loaded.classes.get("plugin_files_list"),
            Some(ToolClass::ReadOnly | ToolClass::ExecuteSafe)
        ));
    }

    #[test]
    fn a_risky_tool_is_reported_and_not_offered() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("plugin.toml");
        std::fs::write(
            &manifest,
            "name = \"clean\"\nversion = \"0.1.0\"\n[[tools]]\nname = \"wipe\"\ndescription = \"Remove the build\"\nprogram = \"rm\"\nargs = [\"-rf\", \"build\"]\ndeclared = \"READ_ONLY\"\n",
        )
        .unwrap();
        let loaded = load(&config_with(&manifest));
        assert!(loaded.tools.is_empty());
        assert!(loaded.classes.is_empty());
        assert!(
            loaded.reports.iter().any(|r| r.contains("not offered")),
            "{:?}",
            loaded.reports
        );
    }

    #[test]
    fn a_missing_or_invalid_manifest_is_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.toml");
        let loaded = load(&config_with(&missing));
        assert!(loaded.tools.is_empty());
        assert!(
            loaded.reports[0].contains("not loaded"),
            "{:?}",
            loaded.reports
        );
    }

    #[test]
    fn the_policy_allows_a_loaded_plugin_tool_and_blocks_an_unknown_one() {
        use harness_core::config::{SecuritySection, Verdict};
        use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("plugin.toml");
        std::fs::write(
            &manifest,
            "name = \"files\"\nversion = \"0.1.0\"\n[[tools]]\nname = \"list\"\ndescription = \"List the root\"\nprogram = \"ls\"\nargs = [\"-1\"]\ndeclared = \"READ_ONLY\"\n",
        )
        .unwrap();
        let loaded = load(&config_with(&manifest));
        let policy = ToolPolicy::new(dir.path(), &SecuritySection::default())
            .unwrap()
            .with_plugin_classes(loaded.classes);
        let known = policy.evaluate(&ToolRequest::new(ToolKind::Harness(
            "plugin_files_list".to_owned(),
        )));
        assert_eq!(known.verdict, Verdict::Allow, "{known:?}");
        let unknown = policy.evaluate(&ToolRequest::new(ToolKind::Harness(
            "plugin_files_wipe".to_owned(),
        )));
        assert_ne!(unknown.verdict, Verdict::Allow, "{unknown:?}");
    }

    #[test]
    fn a_loaded_plugin_tool_runs_its_fixed_command_in_the_workspace() {
        use super::super::test_context;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "here").unwrap();
        let manifest = dir.path().join("plugin.toml");
        std::fs::write(
            &manifest,
            "name = \"files\"\nversion = \"0.1.0\"\n[[tools]]\nname = \"list\"\ndescription = \"List the root\"\nprogram = \"ls\"\nargs = [\"-1\"]\ndeclared = \"READ_ONLY\"\n",
        )
        .unwrap();
        let loaded = load(&config_with(&manifest));
        let ctx = test_context(dir.path());
        let out = loaded.tools[0].call(&json!({}), &ctx).unwrap();
        let structured = out.structured.unwrap();
        assert_eq!(structured["status"], json!("passed"), "{structured}");
        assert!(out.text.contains("marker.txt"), "{}", out.text);
    }
}
