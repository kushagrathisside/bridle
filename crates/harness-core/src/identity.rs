//! The product identity: Bridle.
//!
//! Every user-visible name derived from the product name is defined here and nowhere else. The project's first working
//! name was `local-agent`; its names (`local-agent.toml`, `.local-agent/`, `LOCAL_AGENT_*`, the `local-agent` machine
//! config folder) are the `LEGACY_*` constants below. They are **read as a fallback and never written**, so a project or a
//! machine set up under the old name keeps working without a manual move (ADR-0018). Do not hard-code these strings
//! elsewhere; reference the constants.

use std::path::{Path, PathBuf};

/// Internal product identifier. Used for the MCP server name, the OpenCode provider prefix and logs.
pub const PRODUCT_ID: &str = "bridle";

/// Name of the command-line program. The same program opens the window when it is given no command.
pub const CLI_NAME: &str = "bridle";

/// Per-repository state directory (gitignored by `bridle init`).
pub const STATE_DIR: &str = ".bridle";

/// Per-repository config file name, searched for from the workspace root upwards.
pub const CONFIG_FILE: &str = "bridle.toml";

/// Prefix for every environment variable the harness reads.
pub const ENV_PREFIX: &str = "BRIDLE_";

/// MCP server name registered in OpenCode. OpenCode exposes tools as `<server>_<tool>`.
pub const MCP_SERVER_NAME: &str = PRODUCT_ID;

/// Prefix of the OpenCode provider ids generated for each model slot (`bridle-main`, ...).
pub const PROVIDER_PREFIX: &str = PRODUCT_ID;

/// Name of the machine-level config directory (under the OS config dir).
pub const MACHINE_CONFIG_DIR: &str = PRODUCT_ID;

/// Name of the machine-level config file inside [`MACHINE_CONFIG_DIR`].
pub const MACHINE_CONFIG_FILE: &str = "config.toml";

/// The state directory of the first working name. Read when the new one does not exist; never created.
pub const LEGACY_STATE_DIR: &str = ".local-agent";

/// The repository config file of the first working name. Read when the new one does not exist; never created.
pub const LEGACY_CONFIG_FILE: &str = "local-agent.toml";

/// The environment variable prefix of the first working name. Read when the new variable is not set.
pub const LEGACY_ENV_PREFIX: &str = "LOCAL_AGENT_";

/// The machine config directory of the first working name. Read when the new one has no config file.
pub const LEGACY_MACHINE_CONFIG_DIR: &str = "local-agent";

/// Returns the full environment variable name for a harness setting, e.g. `env_var("CONFIG")` → `BRIDLE_CONFIG`.
pub fn env_var(name: &str) -> String {
    format!("{ENV_PREFIX}{name}")
}

/// The variable of the first working name for a setting, e.g. `legacy_env_var("CONFIG")` → `LOCAL_AGENT_CONFIG`.
pub fn legacy_env_var(name: &str) -> String {
    format!("{LEGACY_ENV_PREFIX}{name}")
}

/// Looks a setting up in `BRIDLE_<name>`, then in `LOCAL_AGENT_<name>`. An empty value counts as not set.
pub fn lookup_env(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(&env_var(name))
        .filter(|value| !value.is_empty())
        .or_else(|| env(&legacy_env_var(name)).filter(|value| !value.is_empty()))
}

/// The harness's state folder for `workspace`: `.bridle`, or the old `.local-agent` when that is the one that exists. A
/// workspace with neither gets `.bridle`.
pub fn state_dir(workspace: &Path) -> PathBuf {
    let current = workspace.join(STATE_DIR);
    let legacy = workspace.join(LEGACY_STATE_DIR);
    if !current.exists() && legacy.is_dir() {
        legacy
    } else {
        current
    }
}

/// The repository config file in `dir`: `bridle.toml`, or the old `local-agent.toml` when only that exists. `None` when
/// neither is a file.
pub fn config_file_in(dir: &Path) -> Option<PathBuf> {
    [CONFIG_FILE, LEGACY_CONFIG_FILE]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Whether `name` is a repository config file name of the product or of its first working name.
pub fn is_config_file_name(name: &std::ffi::OsStr) -> bool {
    name == CONFIG_FILE || name == LEGACY_CONFIG_FILE
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn env_var_is_prefixed() {
        assert_eq!(env_var("CONFIG"), "BRIDLE_CONFIG");
        assert_eq!(legacy_env_var("CONFIG"), "LOCAL_AGENT_CONFIG");
    }

    #[test]
    fn mcp_tool_prefix_matches_opencode_convention() {
        // OpenCode registers MCP tools as "<server>_<tool>".
        assert_eq!(format!("{MCP_SERVER_NAME}_git_status"), "bridle_git_status");
    }

    #[test]
    fn the_new_variable_wins_and_the_old_one_is_the_fallback() {
        let both = |name: &str| match name {
            "BRIDLE_HOME" => Some("new".to_owned()),
            "LOCAL_AGENT_HOME" => Some("old".to_owned()),
            _ => None,
        };
        assert_eq!(lookup_env(&both, "HOME").as_deref(), Some("new"));
        let old_only = |name: &str| (name == "LOCAL_AGENT_HOME").then(|| "old".to_owned());
        assert_eq!(lookup_env(&old_only, "HOME").as_deref(), Some("old"));
        let empty_new = |name: &str| match name {
            "BRIDLE_HOME" => Some(String::new()),
            "LOCAL_AGENT_HOME" => Some("old".to_owned()),
            _ => None,
        };
        assert_eq!(lookup_env(&empty_new, "HOME").as_deref(), Some("old"));
        assert_eq!(lookup_env(&|_: &str| None, "HOME"), None);
    }

    #[test]
    fn the_state_folder_is_the_new_one_unless_only_the_old_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(state_dir(dir.path()), dir.path().join(".bridle"));
        std::fs::create_dir(dir.path().join(".local-agent")).unwrap();
        assert_eq!(state_dir(dir.path()), dir.path().join(".local-agent"));
        std::fs::create_dir(dir.path().join(".bridle")).unwrap();
        assert_eq!(state_dir(dir.path()), dir.path().join(".bridle"));
    }

    #[test]
    fn the_config_file_is_the_new_name_with_the_old_name_as_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(config_file_in(dir.path()), None);
        std::fs::write(dir.path().join("local-agent.toml"), "").unwrap();
        assert_eq!(
            config_file_in(dir.path()),
            Some(dir.path().join("local-agent.toml"))
        );
        std::fs::write(dir.path().join("bridle.toml"), "").unwrap();
        assert_eq!(
            config_file_in(dir.path()),
            Some(dir.path().join("bridle.toml"))
        );
        assert!(is_config_file_name(std::ffi::OsStr::new("bridle.toml")));
        assert!(is_config_file_name(std::ffi::OsStr::new(
            "local-agent.toml"
        )));
        assert!(!is_config_file_name(std::ffi::OsStr::new("config.toml")));
    }
}
