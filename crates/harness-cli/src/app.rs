//! Per-invocation application context: workspace, effective config, and the services built from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use harness_core::config::{
    Config, ConfigIssue, ConfigSources, Origin, Severity, discover_repo_config, load_layered,
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
    /// Which layer set each key (see `LoadedConfig::origins`); used by the TUI only.
    pub origins: BTreeMap<String, Origin>,
}

impl App {
    /// Resolves the workspace, loads the layered config and validates it. Validation issues are
    /// kept (commands decide whether errors are fatal); only unreadable/unparseable config fails.
    pub fn load(explicit_config: Option<&Path>, workspace: Option<&Path>) -> anyhow::Result<Self> {
        let workspace = resolve_workspace(workspace)?;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let explicit = explicit_config
            .map(Path::to_path_buf)
            .or_else(|| identity::lookup_env(&env, "CONFIG").map(PathBuf::from));
        let sources = ConfigSources {
            machine: machine_config_path(),
            repo: discover_repo_config(&workspace),
            explicit,
        };
        let loaded = load_layered(&sources, &env).context("loading configuration")?;
        let issues = validate(&loaded.config);
        // Every remote request of this process takes the route the configuration names.
        harness_runtime::http::set_remote_route(remote_route_of(&loaded.config, &env));
        Ok(Self {
            workspace,
            config: loaded.config,
            config_files: loaded.files,
            issues,
            origins: loaded.origins,
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
        identity::state_dir(&self.workspace)
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
        let policy = ToolPolicy::new(&self.workspace, &self.config.security)
            .with_context(|| format!("building tool policy for {}", self.workspace.display()))?;
        // The classes of the loaded plugin tools, so the policy can tell them apart from unknown tools (ADR-0013).
        Ok(policy
            .with_plugin_classes(harness_runtime::mcp::tools::plugins::load(&self.config).classes))
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

/// The route remote requests take: the `BRIDLE_PROXY` variable when it is set (it may carry credentials, so it is kept
/// apart and never shown), else what `network.proxy_mode` says. A custom mode with an unusable URL is a configuration
/// error (V47) and routes directly, so a mistake never sends traffic somewhere else.
pub fn remote_route_of(
    config: &harness_core::config::Config,
    env: &dyn Fn(&str) -> Option<String>,
) -> harness_runtime::http::RemoteRoute {
    use harness_core::config::ProxyMode;
    use harness_runtime::http::RemoteRoute;
    if let Some(url) = identity::lookup_env(env, "PROXY") {
        return RemoteRoute::Secret(url);
    }
    match config.network.proxy_mode {
        ProxyMode::Environment => RemoteRoute::Environment,
        ProxyMode::None => RemoteRoute::Direct,
        ProxyMode::Custom => match config.network.proxy.as_deref() {
            Some(url) if harness_core::config::check_proxy_url(url).is_ok() => {
                RemoteRoute::Custom(url.to_owned())
            }
            _ => RemoteRoute::Direct,
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod route_tests {
    use super::*;
    use harness_core::config::{Config, ProxyMode};
    use harness_runtime::http::RemoteRoute;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_route_follows_the_configured_mode() {
        let mut config = Config::default();
        assert_eq!(remote_route_of(&config, &no_env), RemoteRoute::Environment);
        config.network.proxy_mode = ProxyMode::None;
        assert_eq!(remote_route_of(&config, &no_env), RemoteRoute::Direct);
        config.network.proxy_mode = ProxyMode::Custom;
        config.network.proxy = Some("http://172.31.2.4:8080".to_owned());
        assert_eq!(
            remote_route_of(&config, &no_env),
            RemoteRoute::Custom("http://172.31.2.4:8080".to_owned())
        );
    }

    #[test]
    fn a_broken_custom_proxy_routes_directly_and_the_variable_wins_over_the_file() {
        let mut config = Config::default();
        config.network.proxy_mode = ProxyMode::Custom;
        config.network.proxy = Some("http://user:pw@proxy:1".to_owned());
        assert_eq!(remote_route_of(&config, &no_env), RemoteRoute::Direct);
        let from_env =
            |name: &str| (name == "BRIDLE_PROXY").then(|| "http://u:p@secret:3128".to_owned());
        assert!(matches!(
            remote_route_of(&config, &from_env),
            RemoteRoute::Secret(_)
        ));
    }
}
