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

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::schema::Config;
use crate::identity;

/// Errors from locating, reading, parsing or assembling configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The explicitly requested config file does not exist.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    /// A config file exists but cannot be read.
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The document is not valid TOML or does not match the [`Config`] schema.
    #[error("invalid config in {path}: {message}")]
    Parse { path: String, message: String },
    /// A repository config sets something only the machine config or an explicit file may set
    /// (see `config::trust`).
    #[error(
        "{path}: a repository config may not set `{key}` ({reason}); move it to the machine config \
         or pass the file with --config"
    )]
    Untrusted {
        path: PathBuf,
        key: String,
        reason: String,
    },
}

/// Which files participate in a layered load. Built by the CLI from flags, env and discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSources {
    /// Machine-level config (see [`machine_config_path`]). Skipped when the file does not exist.
    pub machine: Option<PathBuf>,
    /// Repository config (see [`discover_repo_config`]). Skipped when the file does not exist.
    pub repo: Option<PathBuf>,
    /// File from `--config` or `LOCAL_AGENT_CONFIG`. Must exist.
    pub explicit: Option<PathBuf>,
}

/// Which layer supplied a configuration value. Values with no recorded origin are built-in defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The machine config (trusted).
    Machine,
    /// The repository config (untrusted, restricted by `config::trust`).
    Repository,
    /// An explicit `--config` file or `$LOCAL_AGENT_CONFIG` (trusted).
    Explicit,
    /// An environment override such as `LOCAL_AGENT_MODELS_DIR`.
    Environment,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Repository => "repo",
            Self::Explicit => "--config",
            Self::Environment => "env",
        }
    }
}

/// Deepest key path (`models.main.endpoint` has three parts) for which an origin is recorded.
const ORIGIN_DEPTH: usize = 3;

/// The effective config plus the files it was built from (shown by `agent config show`).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    /// The fully merged and deserialised configuration, with environment overrides applied.
    pub config: Config,
    /// Layers that were read, lowest precedence first.
    pub files: Vec<PathBuf>,
    /// For each dotted key path (up to [`ORIGIN_DEPTH`] parts) the highest-precedence layer that
    /// set it. Informational only: nothing in the loader or the trust check reads it.
    pub origins: BTreeMap<String, Origin>,
}

/// Records `origin` for every key of `table` down to [`ORIGIN_DEPTH`] parts.
fn record_origins(
    table: &toml::Table,
    origin: Origin,
    prefix: &str,
    depth: usize,
    out: &mut BTreeMap<String, Origin>,
) {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        out.insert(path.clone(), origin);
        if let toml::Value::Table(inner) = value
            && depth < ORIGIN_DEPTH
        {
            record_origins(inner, origin, &path, depth + 1, out);
        }
    }
}

/// Parses a complete TOML document into a [`Config`]. Errors are reported against `<string>`.
pub fn from_toml_str(s: &str) -> Result<Config, ConfigError> {
    toml::from_str::<Config>(s).map_err(|err| parse_error("<string>", err))
}

/// Deep-merges `overlay` into `base`. Tables merge key by key; every other value in `overlay`,
/// arrays included, replaces the value in `base`.
pub fn merge_toml(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                merge_toml(base_table, overlay_table);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Finds the nearest repository config: the first `local-agent.toml` that is a regular file in
/// `start` or one of its ancestors.
pub fn discover_repo_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(identity::CONFIG_FILE))
        .find(|candidate| candidate.is_file())
}

/// Location of the machine-level config file. Honours `LOCAL_AGENT_HOME` first, then the
/// platform config directory. The path is returned even if the file does not exist; `None` only
/// when no base directory can be determined.
pub fn machine_config_path() -> Option<PathBuf> {
    machine_config_path_with(&|name: &str| std::env::var(name).ok(), cfg!(windows))
}

/// Loads and merges every layer in `sources`, then applies the environment overrides.
///
/// Optional layers (machine, repo) are skipped when their file is absent. The explicit layer must
/// exist. `env` is consulted for `LOCAL_AGENT_MODELS_DIR` and `LOCAL_AGENT_RUNTIMES_DIR`.
pub fn load_layered(
    sources: &ConfigSources,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<LoadedConfig, ConfigError> {
    let mut merged = toml::Table::new();
    let mut files = Vec::new();
    let mut origins = BTreeMap::new();

    // The repository layer is the only untrusted one: it arrives with the repository.
    for (path, trusted, origin) in [
        (&sources.machine, true, Origin::Machine),
        (&sources.repo, false, Origin::Repository),
    ]
    .into_iter()
    .filter_map(|(path, trusted, origin)| path.as_ref().map(|path| (path, trusted, origin)))
    .filter(|(path, _, _)| path.exists())
    {
        read_layer(path, trusted, origin, &mut merged, &mut files, &mut origins)?;
    }
    if let Some(path) = &sources.explicit {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.clone()));
        }
        read_layer(
            path,
            true,
            Origin::Explicit,
            &mut merged,
            &mut files,
            &mut origins,
        )?;
    }

    let source_label = files
        .last()
        .map_or_else(|| "<merged>".to_owned(), |path| path.display().to_string());
    let mut config = toml::Value::Table(merged)
        .try_into::<Config>()
        .map_err(|err| parse_error(source_label, err))?;

    if let Some(dir) = env_path(env, &identity::env_var("MODELS_DIR")) {
        config.paths.models_dir = Some(dir);
        origins.insert("paths.models_dir".to_owned(), Origin::Environment);
    }
    if let Some(dir) = env_path(env, &identity::env_var("RUNTIMES_DIR")) {
        config.paths.runtimes_dir = Some(dir);
        origins.insert("paths.runtimes_dir".to_owned(), Origin::Environment);
    }

    Ok(LoadedConfig {
        config,
        files,
        origins,
    })
}

/// Reads one layer, merges it into `merged` and records it in `files`. An untrusted layer is
/// first checked against the layers below it (`config::trust`).
fn read_layer(
    path: &Path,
    trusted: bool,
    origin: Origin,
    merged: &mut toml::Table,
    files: &mut Vec<PathBuf>,
    origins: &mut BTreeMap<String, Origin>,
) -> Result<(), ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|err| parse_error(path.display().to_string(), err))?;
    if !trusted {
        super::trust::check_repo_layer(path, &table, merged)?;
    }
    record_origins(&table, origin, "", 1, origins);
    merge_toml(merged, table);
    files.push(path.to_path_buf());
    Ok(())
}

/// Machine config location for an injected environment. `is_windows` selects the Windows rules.
fn machine_config_path_with(
    env: &dyn Fn(&str) -> Option<String>,
    is_windows: bool,
) -> Option<PathBuf> {
    if let Some(home) = env_path(env, &identity::env_var("HOME")) {
        return Some(home.join(identity::MACHINE_CONFIG_FILE));
    }
    let config_dir = if is_windows {
        env_path(env, "APPDATA")?
    } else if let Some(xdg) = env_path(env, "XDG_CONFIG_HOME") {
        xdg
    } else {
        env_path(env, "HOME")?.join(".config")
    };
    Some(
        config_dir
            .join(identity::MACHINE_CONFIG_DIR)
            .join(identity::MACHINE_CONFIG_FILE),
    )
}

/// Reads a non-empty path from the injected environment.
fn env_path(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn parse_error(path: impl Into<String>, err: impl Display) -> ConfigError {
    ConfigError::Parse {
        path: path.into(),
        message: err.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;

    use super::*;
    use crate::config::ProfileKind;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn write_file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn merge_recurses_into_tables_and_replaces_arrays() {
        let mut base: toml::Table = toml::from_str(
            r#"
            [agent]
            max_iterations = 3
            allowed = ["a", "b"]

            [paths]
            models_dir = "/a"
            "#,
        )
        .expect("base parses");
        let overlay: toml::Table = toml::from_str(
            r#"
            [agent]
            max_iterations = 9
            allowed = ["c"]

            [host]
            opencode_version = "x"
            "#,
        )
        .expect("overlay parses");
        let expected: toml::Table = toml::from_str(
            r#"
            [agent]
            max_iterations = 9
            allowed = ["c"]

            [paths]
            models_dir = "/a"

            [host]
            opencode_version = "x"
            "#,
        )
        .expect("expected parses");

        merge_toml(&mut base, overlay);

        assert_eq!(base, expected);
    }

    #[test]
    fn scalar_overlay_replaces_a_table() {
        let mut base: toml::Table = toml::from_str("[a]\nx = 1\n").expect("base parses");
        merge_toml(&mut base, toml::from_str("a = 5").expect("overlay parses"));
        assert_eq!(
            base,
            toml::from_str::<toml::Table>("a = 5").expect("expected")
        );
    }

    #[test]
    fn explicit_overrides_repo_overrides_machine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let machine = write_file(
            dir.path(),
            "machine.toml",
            "profile = \"development\"\n\n[agent]\nmax_iterations = 3\nmax_same_failure = 4\n",
        );
        let repo = write_file(
            dir.path(),
            "local-agent.toml",
            "[agent]\nmax_iterations = 5\n",
        );
        let explicit = write_file(dir.path(), "explicit.toml", "[agent]\nmax_iterations = 7\n");

        let all = ConfigSources {
            machine: Some(machine.clone()),
            repo: Some(repo.clone()),
            explicit: Some(explicit.clone()),
        };
        let loaded = load_layered(&all, &no_env).expect("layers load");
        assert_eq!(loaded.config.agent.max_iterations, 7);
        assert_eq!(loaded.config.agent.max_same_failure, 4);
        assert_eq!(loaded.config.profile, ProfileKind::Development);
        assert_eq!(loaded.files, vec![machine.clone(), repo.clone(), explicit]);

        let without_explicit = ConfigSources {
            explicit: None,
            ..all
        };
        let loaded = load_layered(&without_explicit, &no_env).expect("layers load");
        assert_eq!(loaded.config.agent.max_iterations, 5);
        assert_eq!(loaded.files, vec![machine, repo]);
    }

    #[test]
    fn origins_name_the_highest_precedence_layer_per_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let machine = write_file(
            dir.path(),
            "machine.toml",
            "[agent]\nmax_iterations = 3\nmax_same_failure = 4\n\n[paths]\nmodels_dir = \"/m\"\n",
        );
        let repo = write_file(
            dir.path(),
            "local-agent.toml",
            "[agent]\nmax_iterations = 5\n",
        );
        let explicit = write_file(dir.path(), "explicit.toml", "[agent]\nmax_iterations = 7\n");
        let sources = ConfigSources {
            machine: Some(machine),
            repo: Some(repo),
            explicit: Some(explicit),
        };
        let env = |name: &str| (name == "LOCAL_AGENT_MODELS_DIR").then(|| "/env".to_owned());
        let loaded = load_layered(&sources, &env).expect("loads");
        let o = &loaded.origins;
        assert_eq!(o["agent.max_iterations"], Origin::Explicit);
        assert_eq!(o["agent.max_same_failure"], Origin::Machine);
        assert_eq!(o["paths.models_dir"], Origin::Environment);
        assert_eq!(
            o["agent"],
            Origin::Explicit,
            "the section names its last writer"
        );
        assert!(
            !o.contains_key("agent.max_command_retries"),
            "defaults have no origin"
        );

        let repo_only = ConfigSources {
            repo: sources.repo.clone(),
            ..Default::default()
        };
        let loaded = load_layered(&repo_only, &no_env).expect("loads");
        assert_eq!(loaded.origins["agent.max_iterations"], Origin::Repository);
    }

    #[test]
    fn origins_stop_at_three_key_parts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let machine = write_file(
            dir.path(),
            "machine.toml",
            "[models.main]\nprovider = \"mock\"\nid = \"m\"\nrole = \"engineer\"\nendpoint = \"http://127.0.0.1:1/v1\"\ncontext = 4096\nhardware = \"cpu\"\n[models.main.sampling]\ntemperature = 0.5\n",
        );
        let sources = ConfigSources {
            machine: Some(machine),
            ..Default::default()
        };
        let o = load_layered(&sources, &no_env).expect("loads").origins;
        assert_eq!(o["models.main.endpoint"], Origin::Machine);
        assert_eq!(o["models.main.sampling"], Origin::Machine);
        assert!(!o.contains_key("models.main.sampling.temperature"));
    }

    #[test]
    fn environment_overrides_paths_and_ignores_empty_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let explicit = write_file(
            dir.path(),
            "config.toml",
            "[paths]\nmodels_dir = \"/from/file\"\n",
        );
        let sources = ConfigSources {
            explicit: Some(explicit),
            ..Default::default()
        };

        let env = |name: &str| match name {
            "LOCAL_AGENT_MODELS_DIR" => Some("/mnt/models".to_owned()),
            "LOCAL_AGENT_RUNTIMES_DIR" => Some("/mnt/runtimes".to_owned()),
            _ => None,
        };
        let loaded = load_layered(&sources, &env).expect("loads");
        assert_eq!(
            loaded.config.paths.models_dir,
            Some(PathBuf::from("/mnt/models"))
        );
        assert_eq!(
            loaded.config.paths.runtimes_dir,
            Some(PathBuf::from("/mnt/runtimes"))
        );

        let empty = |_: &str| Some(String::new());
        let loaded = load_layered(&sources, &empty).expect("loads");
        assert_eq!(
            loaded.config.paths.models_dir,
            Some(PathBuf::from("/from/file"))
        );
        assert_eq!(loaded.config.paths.runtimes_dir, None);
    }

    #[test]
    fn unknown_key_is_a_parse_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bad = write_file(dir.path(), "bad.toml", "[agent]\nmax_iterationz = 3\n");
        let sources = ConfigSources {
            explicit: Some(bad),
            ..Default::default()
        };

        match load_layered(&sources, &no_env) {
            Err(ConfigError::Parse { path, message }) => {
                assert!(path.ends_with("bad.toml"), "path was {path}");
                assert!(message.contains("max_iterationz"), "message was {message}");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn missing_explicit_file_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("absent.toml");
        let sources = ConfigSources {
            explicit: Some(missing.clone()),
            ..Default::default()
        };

        match load_layered(&sources, &no_env) {
            Err(ConfigError::NotFound(path)) => assert_eq!(path, missing),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn repository_layer_may_not_choose_executables_but_an_explicit_file_may() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = "[runtimes.r]\nkind = \"llama_cpp\"\nserver_bin = \"/usr/bin/evil\"\n";
        let repo = write_file(dir.path(), "local-agent.toml", body);
        let from_repo = ConfigSources {
            repo: Some(repo.clone()),
            ..ConfigSources::default()
        };
        match load_layered(&from_repo, &no_env) {
            Err(ConfigError::Untrusted { path, key, .. }) => {
                assert_eq!(path, repo);
                assert_eq!(key, "runtimes.r.server_bin");
            }
            other => panic!("expected Untrusted, got {other:?}"),
        }

        let explicit = ConfigSources {
            explicit: Some(repo),
            ..ConfigSources::default()
        };
        let loaded = load_layered(&explicit, &no_env).expect("an explicit file is trusted");
        assert!(loaded.config.runtimes.contains_key("r"));
    }

    #[test]
    fn missing_machine_and_repo_files_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sources = ConfigSources {
            machine: Some(dir.path().join("machine.toml")),
            repo: Some(dir.path().join("local-agent.toml")),
            explicit: None,
        };

        let loaded = load_layered(&sources, &no_env).expect("loads");
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn empty_sources_yield_defaults() {
        let loaded = load_layered(&ConfigSources::default(), &no_env).expect("loads");
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn discovery_walks_up_to_the_nearest_config_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).expect("dirs");

        let outer = write_file(root.path(), identity::CONFIG_FILE, "");
        assert_eq!(discover_repo_config(&nested), Some(outer.clone()));

        let inner = write_file(&root.path().join("a"), identity::CONFIG_FILE, "");
        assert_eq!(discover_repo_config(&nested), Some(inner));
        assert_eq!(discover_repo_config(root.path()), Some(outer));
    }

    #[test]
    fn discovery_ignores_directories_named_like_the_config() {
        let root = tempfile::tempdir().expect("tempdir");
        let outer = write_file(root.path(), identity::CONFIG_FILE, "");
        fs::create_dir_all(root.path().join("x").join(identity::CONFIG_FILE)).expect("dirs");

        assert_eq!(discover_repo_config(&root.path().join("x")), Some(outer));
    }

    #[test]
    fn machine_path_prefers_local_agent_home() {
        let env = |name: &str| match name {
            "LOCAL_AGENT_HOME" => Some("/opt/la".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        assert_eq!(
            machine_config_path_with(&env, false),
            Some(PathBuf::from("/opt/la/config.toml"))
        );
    }

    #[test]
    fn empty_local_agent_home_is_ignored() {
        let env = |name: &str| match name {
            "LOCAL_AGENT_HOME" => Some(String::new()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        assert_eq!(
            machine_config_path_with(&env, false),
            Some(PathBuf::from("/home/u/.config/local-agent/config.toml"))
        );
    }

    #[test]
    fn xdg_config_home_is_used_before_home() {
        let env = |name: &str| match name {
            "XDG_CONFIG_HOME" => Some("/xdg".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        assert_eq!(
            machine_config_path_with(&env, false),
            Some(PathBuf::from("/xdg/local-agent/config.toml"))
        );
    }

    #[test]
    fn windows_machine_path_uses_appdata() {
        let env = |name: &str| match name {
            "APPDATA" => Some(r"C:\Users\u\AppData\Roaming".to_owned()),
            _ => None,
        };
        assert_eq!(
            machine_config_path_with(&env, true),
            Some(
                PathBuf::from(r"C:\Users\u\AppData\Roaming")
                    .join("local-agent")
                    .join("config.toml")
            )
        );
    }

    #[test]
    fn machine_path_is_none_without_a_base_directory() {
        assert_eq!(machine_config_path_with(&no_env, false), None);
        assert_eq!(machine_config_path_with(&no_env, true), None);
    }

    #[test]
    fn from_toml_str_accepts_partial_documents() {
        let cfg = from_toml_str("profile = \"test\"\n").expect("parses");
        assert_eq!(
            cfg,
            Config {
                profile: ProfileKind::Test,
                ..Config::default()
            }
        );
    }

    #[test]
    fn from_toml_str_reports_errors_against_string_source() {
        match from_toml_str("[agent]\nmax_iterationz = 3\n") {
            Err(ConfigError::Parse { path, message }) => {
                assert_eq!(path, "<string>");
                assert!(message.contains("max_iterationz"), "message was {message}");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
        assert!(matches!(
            from_toml_str("[agent"),
            Err(ConfigError::Parse { .. })
        ));
    }
}
