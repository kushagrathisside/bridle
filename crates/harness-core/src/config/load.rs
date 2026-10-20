//! Layered config loading. See the module docs in `config/mod.rs` for the layer order.
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `from_toml_str`: parse a full document into [`Config`]; map TOML errors to
//!   [`ConfigError::Parse`] with the message.
//! - `merge_toml(base, overlay)`: deep-merge `overlay` into `base`. Tables merge recursively;
//!   every other value type (arrays included) in `overlay` replaces the value in `base`.
//! - `discover_repo_config(start)`: walk from `start` up through its ancestors; return the first
//!   `<dir>/local-agent.toml` (use `identity::CONFIG_FILE`) that is a file. `None` if none.
//! - `machine_config_path()`: if env `LOCAL_AGENT_HOME` (use `identity::env_var("HOME")`) is set
//!   and non-empty → `<it>/config.toml`. Else on Windows `%APPDATA%\local-agent\config.toml`;
//!   else `$XDG_CONFIG_HOME/local-agent/config.toml` or `$HOME/.config/local-agent/config.toml`.
//!   Use `identity::MACHINE_CONFIG_DIR` / `MACHINE_CONFIG_FILE`. Returns the path even if the file
//!   does not exist; `None` only if no base directory can be determined.
//! - `load_layered(sources, env)`: start from an empty `toml::Table`; for each of
//!   `sources.machine`, `sources.repo`, `sources.explicit` that is `Some` *and exists* (explicit
//!   must exist, otherwise `ConfigError::NotFound`), read it (`ConfigError::Io`), parse to a
//!   `toml::Table` (`ConfigError::Parse { path, message }`) and `merge_toml` it in, recording the
//!   path in `LoadedConfig::files`. Deserialize the merged table into `Config`
//!   (`ConfigError::Parse` with path = last file or "<merged>"). Then apply env overrides from the
//!   `env` closure: `LOCAL_AGENT_MODELS_DIR` → `paths.models_dir`, `LOCAL_AGENT_RUNTIMES_DIR` →
//!   `paths.runtimes_dir` (non-empty values only). The `env` closure is injected so tests never
//!   mutate the real process environment.
//! - Unit tests: merge semantics (nested tables merge, arrays replace), discovery walks up,
//!   precedence explicit > repo > machine, env override, unknown key is a parse error,
//!   explicit-missing is NotFound, empty sources yields `Config::default()`.

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::schema::Config;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config in {path}: {message}")]
    Parse { path: String, message: String },
}

/// Which files participate in a layered load. Built by the CLI from flags, env and discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSources {
    pub machine: Option<PathBuf>,
    pub repo: Option<PathBuf>,
    pub explicit: Option<PathBuf>,
}

/// The effective config plus the files it was built from (shown by `agent config show`).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    pub files: Vec<PathBuf>,
}

pub fn from_toml_str(s: &str) -> Result<Config, ConfigError> {
    let _ = s;
    todo!("parse TOML into Config")
}

pub fn merge_toml(base: &mut toml::Table, overlay: toml::Table) {
    let _ = (base, overlay);
    todo!("deep merge")
}

pub fn discover_repo_config(start: &Path) -> Option<PathBuf> {
    let _ = start;
    todo!("walk up looking for local-agent.toml")
}

pub fn machine_config_path() -> Option<PathBuf> {
    todo!("machine config location")
}

pub fn load_layered(
    sources: &ConfigSources,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<LoadedConfig, ConfigError> {
    let _ = (sources, env);
    todo!("layered load")
}
