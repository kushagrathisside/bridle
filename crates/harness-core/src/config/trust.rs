//! What an untrusted repository config may set.
//!
//! A repository's `local-agent.toml` arrives with the repository, so a cloned repository can carry
//! a hostile one. The machine config and an explicit `--config` file are chosen by the user and
//! are trusted. The repository layer may tune agent behaviour and declare verification steps (each
//! step is classified before it runs). It may not choose executables or locations outside the
//! configured directories, name credentials, point a model at a non-loopback host, enable
//! external providers, or loosen security.

use std::path::Path;

use toml::{Table, Value};

use super::is_absolute_any_platform;
use super::load::ConfigError;
use super::schema::{SecuritySection, Verdict};
use super::validate::{endpoint_host, is_loopback_host};

/// Profiles that send work off the machine; only the machine owner may choose them.
const REMOTE_PROFILES: &[&str] = &["hybrid", "local-with-jev"];

/// Checks the repository layer `repo` (read from `path`) against the layers below it, `lower`.
/// Returns the first setting the repository may not make.
pub(super) fn check_repo_layer(
    path: &Path,
    repo: &Table,
    lower: &Table,
) -> Result<(), ConfigError> {
    let deny = |key: String, reason: &str| ConfigError::Untrusted {
        path: path.to_path_buf(),
        key,
        reason: reason.to_owned(),
    };

    if repo.contains_key("tools") {
        return Err(deny(
            "tools".to_owned(),
            "a repository may not add tools; declare adapters in the machine config or a --config file",
        ));
    }
    if repo.contains_key("paths") {
        return Err(deny(
            "paths".to_owned(),
            "model and runtime locations are machine settings",
        ));
    }
    if table(repo, "host").is_some_and(|host| host.contains_key("opencode_bin")) {
        return Err(deny(
            "host.opencode_bin".to_owned(),
            "the host executable is a machine setting",
        ));
    }
    if table(repo, "sandbox")
        .and_then(|sandbox| sandbox.get("enabled"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(deny(
            "sandbox.enabled".to_owned(),
            "a repository may not switch the sandbox off; only the machine config may set enabled = false",
        ));
    }
    if table(repo, "workflow")
        .and_then(|workflow| workflow.get("enabled"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(deny(
            "workflow.enabled".to_owned(),
            "a repository may enable the workflow but not switch it off; only the machine config may set enabled = false",
        ));
    }
    if let Some(profile) = repo.get("profile").and_then(Value::as_str)
        && REMOTE_PROFILES.contains(&profile)
    {
        return Err(deny(
            "profile".to_owned(),
            "this profile sends work off the machine",
        ));
    }

    for (name, runtime) in tables(repo, "runtimes") {
        if let Some(bin) = runtime.get("server_bin").and_then(Value::as_str)
            && !is_contained_relative(bin)
        {
            return Err(deny(
                format!("runtimes.{name}.server_bin"),
                "a repository may only name a binary inside the configured runtimes directory",
            ));
        }
    }

    for (slot, model) in tables(repo, "models") {
        if model.contains_key("api_key_env") {
            return Err(deny(
                format!("models.{slot}.api_key_env"),
                "credentials are machine settings",
            ));
        }
        if let Some(endpoint) = model.get("endpoint").and_then(Value::as_str)
            && !endpoint_host(endpoint).is_some_and(is_loopback_host)
        {
            return Err(deny(
                format!("models.{slot}.endpoint"),
                "a repository may only point models at this machine (loopback)",
            ));
        }
        if let Some(file) = table(model, "launch")
            .and_then(|launch| launch.get("model_file"))
            .and_then(Value::as_str)
            && !is_contained_relative(file)
        {
            return Err(deny(
                format!("models.{slot}.launch.model_file"),
                "a repository may only name a file inside the configured models directory",
            ));
        }
    }

    for (name, external) in tables(repo, "external") {
        for (key, value) in external {
            if !(key == "enabled" && value.as_bool() == Some(false)) {
                return Err(deny(
                    format!("external.{name}.{key}"),
                    "external providers are machine settings (a repository may only disable them)",
                ));
            }
        }
    }

    if let Some(security) = table(repo, "security") {
        check_security(security, table(lower, "security"), &deny)?;
    }
    Ok(())
}

/// Security settings may only get stricter than the layers below (or the defaults).
fn check_security(
    repo: &Table,
    lower: Option<&Table>,
    deny: &dyn Fn(String, &str) -> ConfigError,
) -> Result<(), ConfigError> {
    for key in ["allowed_paths", "safe_commands"] {
        if repo
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
        {
            return Err(deny(
                format!("security.{key}"),
                "a repository may not widen what the policy allows",
            ));
        }
    }

    let defaults = SecuritySection::default();
    let lower_bool = |key: &str, default: bool| {
        lower
            .and_then(|l| l.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    for (key, default) in [
        ("secrets_redaction", defaults.secrets_redaction),
        ("require_worktree", defaults.require_worktree),
    ] {
        if repo.get(key).and_then(Value::as_bool) == Some(false) && lower_bool(key, default) {
            return Err(deny(
                format!("security.{key}"),
                "a repository may not turn off a protection",
            ));
        }
    }

    for (key, default) in [
        ("network", defaults.network),
        ("destructive_commands", defaults.destructive_commands),
        ("privileged_commands", defaults.privileged_commands),
        ("risky_commands", defaults.risky_commands),
    ] {
        let Some(wanted) = repo.get(key).and_then(verdict) else {
            continue;
        };
        let current = lower
            .and_then(|l| l.get(key))
            .and_then(verdict)
            .unwrap_or(default);
        if wanted < current {
            return Err(deny(
                format!("security.{key}"),
                "a repository may make a verdict stricter, never looser",
            ));
        }
    }
    Ok(())
}

/// A path that stays inside the directory it is resolved against: relative, no `..`, no root,
/// drive or expansion. Both separator styles count, so a Windows path is judged the same on Linux.
fn is_contained_relative(text: &str) -> bool {
    !text.is_empty()
        && !is_absolute_any_platform(Path::new(text))
        && !text.starts_with(['/', '\\', '~', '$', '%'])
        && !text.contains(':')
        && !text.split(['/', '\\']).any(|part| part == "..")
}

fn verdict(value: &Value) -> Option<Verdict> {
    value.clone().try_into().ok()
}

fn table<'a>(parent: &'a Table, key: &str) -> Option<&'a Table> {
    parent.get(key).and_then(Value::as_table)
}

/// The sub-tables of `parent[key]` (for `[runtimes.<name>]`, `[models.<slot>]`, ...).
fn tables<'a>(parent: &'a Table, key: &str) -> impl Iterator<Item = (&'a String, &'a Table)> {
    table(parent, key)
        .into_iter()
        .flat_map(|t| t.iter())
        .filter_map(|(name, value)| value.as_table().map(|t| (name, t)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn check(repo: &str, lower: &str) -> Result<(), String> {
        let repo: Table = toml::from_str(repo).expect("repo parses");
        let lower: Table = toml::from_str(lower).expect("lower parses");
        check_repo_layer(Path::new("local-agent.toml"), &repo, &lower).map_err(|e| e.to_string())
    }

    fn rejected_key(repo: &str) -> String {
        check(repo, "").expect_err("must be rejected")
    }

    #[test]
    fn shipped_profiles_are_valid_repository_configs() {
        for profile in [
            include_str!("../../../../configs/local-only.toml"),
            include_str!("../../../../configs/development.toml"),
        ] {
            assert_eq!(check(profile, ""), Ok(()));
        }
    }

    #[test]
    fn executables_and_locations_are_machine_settings() {
        for (repo, key) in [
            ("[paths]\nmodels_dir = \"/tmp\"", "paths"),
            (
                "[host]\nopencode_bin = \"/x/opencode\"",
                "host.opencode_bin",
            ),
            (
                "[runtimes.r]\nkind = \"llama_cpp\"\nserver_bin = \"/usr/bin/evil\"",
                "runtimes.r.server_bin",
            ),
            (
                "[runtimes.r]\nserver_bin = 'C:\\Windows\\System32\\cmd.exe'",
                "runtimes.r.server_bin",
            ),
            (
                "[runtimes.r]\nserver_bin = '\\Windows\\cmd.exe'",
                "runtimes.r.server_bin",
            ),
            (
                "[runtimes.r]\nserver_bin = '..\\..\\x.exe'",
                "runtimes.r.server_bin",
            ),
            (
                "[runtimes.r]\nserver_bin = 'C:x.exe'",
                "runtimes.r.server_bin",
            ),
            ("[runtimes.r]\nserver_bin = '~/x'", "runtimes.r.server_bin"),
            (
                "[runtimes.r]\nserver_bin = '%TEMP%/x.exe'",
                "runtimes.r.server_bin",
            ),
            (
                "[models.m.launch]\nmodel_file = \"../../secret.gguf\"",
                "models.m.launch.model_file",
            ),
        ] {
            let message = rejected_key(repo);
            assert!(message.contains(key), "{repo}: {message}");
            assert!(message.contains("machine config"), "{message}");
        }
    }

    #[test]
    fn credentials_remote_endpoints_and_external_providers_are_rejected() {
        for (repo, key) in [
            (
                "[models.m]\napi_key_env = \"AWS_SECRET_ACCESS_KEY\"",
                "models.m.api_key_env",
            ),
            (
                "[models.m]\nendpoint = \"http://10.0.0.5:8001/v1\"",
                "models.m.endpoint",
            ),
            (
                "[models.m]\nendpoint = \"https://api.example.com/v1\"",
                "models.m.endpoint",
            ),
            ("profile = \"hybrid\"", "profile"),
            ("profile = \"local-with-jev\"", "profile"),
            ("[external.jev]\nenabled = true", "external.jev.enabled"),
            (
                "[external.jev]\nendpoint = \"https://x\"",
                "external.jev.endpoint",
            ),
        ] {
            let message = rejected_key(repo);
            assert!(message.contains(key), "{repo}: {message}");
        }
        assert_eq!(
            check(
                "[models.m]\nendpoint = \"http://[::1]:8001/v1\"\n[external.jev]\nenabled = false",
                ""
            ),
            Ok(())
        );
    }

    #[test]
    fn a_repository_may_not_switch_the_sandbox_off() {
        let message = rejected_key("[sandbox]\nenabled = false");
        assert!(message.contains("sandbox.enabled"), "{message}");
        assert_eq!(
            check("[sandbox]\nenabled = true\nnetwork = false", ""),
            Ok(())
        );
        assert_eq!(
            check("[sandbox]\nnetwork = true", "[sandbox]\nenabled = false"),
            Ok(())
        );
    }

    #[test]
    fn a_repository_may_enable_the_workflow_but_not_switch_it_off() {
        let message = rejected_key("[workflow]\nenabled = false");
        assert!(message.contains("workflow.enabled"), "{message}");
        assert_eq!(
            check("[workflow]\nenabled = true\nrefusal_cap = 2", ""),
            Ok(())
        );
        assert_eq!(
            check("[workflow]\nrefusal_cap = 2", "[workflow]\nenabled = true"),
            Ok(())
        );
    }

    #[test]
    fn security_may_only_get_stricter() {
        for (repo, key) in [
            (
                "[security]\nrisky_commands = \"allow\"",
                "security.risky_commands",
            ),
            (
                "[security]\nprivileged_commands = \"ask\"",
                "security.privileged_commands",
            ),
            (
                "[security]\nsafe_commands = [\"make\"]",
                "security.safe_commands",
            ),
            (
                "[security]\nallowed_paths = [\"/\"]",
                "security.allowed_paths",
            ),
            (
                "[security]\nsecrets_redaction = false",
                "security.secrets_redaction",
            ),
            (
                "[security]\nrequire_worktree = false",
                "security.require_worktree",
            ),
        ] {
            let message = rejected_key(repo);
            assert!(message.contains(key), "{repo}: {message}");
        }
        // Stricter, equal, or empty lists are fine.
        assert_eq!(
            check(
                "[security]\nnetwork = \"deny\"\nrisky_commands = \"ask\"\nsafe_commands = []",
                ""
            ),
            Ok(())
        );
        // The comparison is against the machine layer, not only the defaults.
        let machine = "[security]\nnetwork = \"allow\"\nrequire_worktree = false";
        assert_eq!(
            check(
                "[security]\nnetwork = \"ask\"\nrequire_worktree = false",
                machine
            ),
            Ok(())
        );
        assert!(check("[security]\nnetwork = \"deny\"", machine).is_ok());
        let strict_machine = "[security]\nnetwork = \"deny\"";
        assert!(check("[security]\nnetwork = \"ask\"", strict_machine).is_err());
    }
}
