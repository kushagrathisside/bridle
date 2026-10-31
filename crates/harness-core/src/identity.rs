//! The temporary product identity.
//!
//! The final product name has not been chosen. Every user-visible name derived from it is defined
//! here and nowhere else, so a rename is a one-file change (plus the `[[bin]]` name in
//! `crates/harness-cli/Cargo.toml` and the shim's `package.json`). Do not hard-code these strings
//! elsewhere; reference the constants.

/// Internal product identifier. Used for the MCP server name, the OpenCode provider prefix and logs.
pub const PRODUCT_ID: &str = "local-agent";

/// Name of the command-line binary.
pub const CLI_NAME: &str = "agent";

/// Per-repository state directory (gitignored by `agent init`).
pub const STATE_DIR: &str = ".local-agent";

/// Per-repository config file name, searched for from the workspace root upwards.
pub const CONFIG_FILE: &str = "local-agent.toml";

/// Prefix for every environment variable the harness reads.
pub const ENV_PREFIX: &str = "LOCAL_AGENT_";

/// MCP server name registered in OpenCode. OpenCode exposes tools as `<server>_<tool>`.
pub const MCP_SERVER_NAME: &str = PRODUCT_ID;

/// Prefix of the OpenCode provider ids generated for each model slot (`local-agent-main`, ...).
pub const PROVIDER_PREFIX: &str = PRODUCT_ID;

/// Returns the full environment variable name for a harness setting, e.g. `env_var("CONFIG")`
/// → `LOCAL_AGENT_CONFIG`.
pub fn env_var(name: &str) -> String {
    format!("{ENV_PREFIX}{name}")
}

/// Name of the machine-level config directory (under the OS config dir).
pub const MACHINE_CONFIG_DIR: &str = PRODUCT_ID;

/// Name of the machine-level config file inside [`MACHINE_CONFIG_DIR`].
pub const MACHINE_CONFIG_FILE: &str = "config.toml";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_is_prefixed() {
        assert_eq!(env_var("CONFIG"), "LOCAL_AGENT_CONFIG");
    }

    #[test]
    fn mcp_tool_prefix_matches_opencode_convention() {
        // OpenCode registers MCP tools as "<server>_<tool>".
        assert_eq!(
            format!("{MCP_SERVER_NAME}_git_status"),
            "local-agent_git_status"
        );
    }
}
