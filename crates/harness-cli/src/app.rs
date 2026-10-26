//! Per-invocation application context: workspace, effective config, and the services built from it.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use harness_core::config::{
    Config, ConfigIssue, ConfigSources, Severity, discover_repo_config, load_layered,
    machine_config_path, validate,
};
use harness_core::identity;
use harness_core::security::tool_policy::ToolPolicy;
use harness_runtime::event_log::EventLog;

#[derive(Debug)]
pub struct App {
    pub workspace: PathBuf,
    pub config: Config,
    pub config_files: Vec<PathBuf>,
    pub issues: Vec<ConfigIssue>,
}

impl App {
    /// Resolves the workspace, loads the layered config and validates it. Validation issues are
    /// kept (commands decide whether errors are fatal); only unreadable/unparseable config fails.
    pub fn load(explicit_config: Option<&Path>, workspace: Option<&Path>) -> anyhow::Result<Self> {
        let workspace = resolve_workspace(workspace)?;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let explicit = explicit_config
            .map(Path::to_path_buf)
            .or_else(|| env(&identity::env_var("CONFIG")).map(PathBuf::from));
        let sources = ConfigSources {
            machine: machine_config_path(),
            repo: discover_repo_config(&workspace),
            explicit,
        };
        let loaded = load_layered(&sources, &env).context("loading configuration")?;
        let issues = validate(&loaded.config);
        Ok(Self {
            workspace,
            config: loaded.config,
            config_files: loaded.files,
            issues,
        })
    }

    pub fn has_config_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    /// Fails with a readable list when the config has errors.
    pub fn require_valid_config(&self) -> anyhow::Result<()> {
        if !self.has_config_errors() {
            return Ok(());
        }
        let list: Vec<String> = self
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("  [{}] {} - {}", i.rule, i.path, i.message))
            .collect();
        bail!(
            "configuration has errors (run `{} config validate`):\n{}",
            identity::CLI_NAME,
            list.join("\n")
        )
    }

    pub fn state_dir(&self) -> PathBuf {
        self.workspace.join(identity::STATE_DIR)
    }

    pub fn event_log(&self) -> EventLog {
        let t = &self.config.telemetry;
        EventLog::for_workspace(
            &self.workspace,
            self.config.security.secrets_redaction,
            t.max_field_bytes,
            t.enabled,
        )
    }

    pub fn policy(&self) -> anyhow::Result<ToolPolicy> {
        ToolPolicy::new(&self.workspace, &self.config.security)
            .with_context(|| format!("building tool policy for {}", self.workspace.display()))
    }
}

/// Canonicalises the workspace and refuses filesystem roots: a root workspace would make the path
/// guard allow everything.
pub fn resolve_workspace(workspace: Option<&Path>) -> anyhow::Result<PathBuf> {
    let raw = match workspace {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("reading current directory")?,
    };
    let canonical = dunce::canonicalize(&raw)
        .with_context(|| format!("workspace {} does not exist", raw.display()))?;
    if canonical.parent().is_none() {
        bail!(
            "refusing to use filesystem root {} as the workspace",
            canonical.display()
        );
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_root_is_rejected() {
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        assert!(resolve_workspace(Some(&root)).is_err());
    }

    #[test]
    fn workspace_is_canonicalised() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a");
        std::fs::create_dir(&nested).expect("mkdir");
        let ws = resolve_workspace(Some(&nested.join(".."))).expect("resolves");
        assert_eq!(ws, dunce::canonicalize(dir.path()).expect("canonical"));
    }
}
