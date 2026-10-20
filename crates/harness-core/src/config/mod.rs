//! Configuration: schema, layered loading and validation.
//!
//! Layers, lowest to highest precedence (each layer deep-merges over the previous one):
//!
//! 1. built-in defaults (serde `Default`s in [`schema`]),
//! 2. the machine config (`%APPDATA%\local-agent\config.toml`, `~/.config/local-agent/config.toml`,
//!    or `$LOCAL_AGENT_HOME/config.toml`) — machine facts such as model and runtime paths,
//! 3. the repository config (`local-agent.toml`, found by walking up from the workspace),
//! 4. an explicit `--config` file or `$LOCAL_AGENT_CONFIG`,
//! 5. environment overrides for paths (`LOCAL_AGENT_MODELS_DIR`, `LOCAL_AGENT_RUNTIMES_DIR`).

mod load;
mod schema;
mod validate;

pub use load::{
    ConfigError, ConfigSources, LoadedConfig, discover_repo_config, from_toml_str, load_layered,
    machine_config_path, merge_toml,
};
pub use schema::*;
pub use validate::{ConfigIssue, Severity, ValidationEnv, validate, validate_with};

use std::path::{Path, PathBuf};

impl Config {
    /// Resolves `models.<slot>.launch.model_file` against `paths.models_dir`.
    ///
    /// Returns `None` when the slot or its launch section is missing, or when the file is relative
    /// and no models directory is configured.
    pub fn resolve_model_file(&self, slot: &str) -> Option<PathBuf> {
        let launch = self.models.get(slot)?.launch.as_ref()?;
        resolve_against(&launch.model_file, self.paths.models_dir.as_deref())
    }

    /// Resolves `runtimes.<name>.server_bin` against `paths.runtimes_dir`.
    pub fn resolve_runtime_bin(&self, runtime: &str) -> Option<PathBuf> {
        let rt = self.runtimes.get(runtime)?;
        resolve_against(&rt.server_bin, self.paths.runtimes_dir.as_deref())
    }
}

fn resolve_against(path: &Path, base: Option<&Path>) -> Option<PathBuf> {
    if is_absolute_any_platform(path) {
        Some(path.to_path_buf())
    } else {
        base.map(|b| b.join(path))
    }
}

/// `Path::is_absolute` is platform-specific; config files written on Windows (`D:\models`) must
/// still be recognised as absolute when validated or tested on Linux.
pub fn is_absolute_any_platform(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || s.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_drive_paths_are_absolute_everywhere() {
        assert!(is_absolute_any_platform(Path::new(r"D:\models\x.gguf")));
        assert!(is_absolute_any_platform(Path::new("D:/models/x.gguf")));
        assert!(!is_absolute_any_platform(Path::new(r"Qwen3.5-9B\x.gguf")));
    }

    #[test]
    fn model_file_resolves_against_models_dir() {
        let mut cfg = Config::default();
        cfg.paths.models_dir = Some(PathBuf::from("/data/models"));
        cfg.models.insert(
            "main".into(),
            from_toml_str(
                r#"
                [models.main]
                provider = "llama_cpp"
                id = "m"
                role = "engineer"
                endpoint = "http://127.0.0.1:8001/v1"
                context = 4096
                hardware = "gpu"
                [models.main.launch]
                runtime = "llama"
                model_file = "Qwen/q.gguf"
                "#,
            )
            .expect("valid toml")
            .models["main"]
                .clone(),
        );
        assert_eq!(
            cfg.resolve_model_file("main"),
            Some(PathBuf::from("/data/models/Qwen/q.gguf"))
        );
        assert_eq!(cfg.resolve_model_file("missing"), None);
    }
}
