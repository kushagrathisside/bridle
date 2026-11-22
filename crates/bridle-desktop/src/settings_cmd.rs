//! The commands of the settings editor: the list of settings (read from the schema), the values the two files set, and
//! changing or resetting one. A change is written into the file with its comments kept, then the configuration is
//! loaded again; if that fails, or names the changed key as an error, the file is put back as it was.

use std::fs;
use std::path::{Path, PathBuf};

use harness_core::config::{self, Severity, settings};
use harness_core::identity;
use serde_json::{Map, Value, json};
use tauri::State;

use crate::bridge::{Bridge, workspace_of};

const MACHINE_HEADER: &str = "# Machine-level config: facts about this computer (model and runtime locations, the proxy).\n\
                              # Managed by `bridle init --machine` and the app; repository settings belong in bridle.toml.\n\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The machine config: every project on this computer.
    User,
    /// The project's `bridle.toml`.
    Workspace,
}

fn scope_of(text: &str) -> Result<Scope, String> {
    match text {
        "user" => Ok(Scope::User),
        "workspace" => Ok(Scope::Workspace),
        other => Err(format!("unknown scope `{other}` (user or workspace)")),
    }
}

/// Where the user file is written and where it is read from (the old name, until the first save moves it).
fn user_paths() -> Result<(PathBuf, PathBuf), String> {
    let write = config::machine_config_write_path()
        .ok_or("cannot determine the machine config location (set BRIDLE_HOME)")?;
    let read = config::machine_config_path().unwrap_or_else(|| write.clone());
    Ok((write, read))
}

fn workspace_file(bridge: &Bridge) -> Result<PathBuf, String> {
    let workspace = workspace_of(&bridge.args()).map_err(|error| format!("{error:#}"))?;
    Ok(identity::config_file_in(&workspace)
        .unwrap_or_else(|| workspace.join(identity::CONFIG_FILE)))
}

fn read_text(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// The values a file sets, by key.
fn values_in(text: &str) -> Value {
    let mut out = Map::new();
    for setting in settings::manifest() {
        if let Some(value) = settings::value_in(text, &setting.key) {
            out.insert(setting.key.clone(), value);
        }
    }
    Value::Object(out)
}

/// The sections and the settings, for the editor to draw.
#[tauri::command]
pub fn settings_manifest() -> Value {
    json!({
        "sections": settings::SECTIONS
            .iter()
            .map(|(key, title, blurb)| json!({"key": key, "title": title, "blurb": blurb}))
            .collect::<Vec<_>>(),
        "settings": settings::manifest(),
    })
}

/// What each file sets, and the value in force now (all layers merged).
#[tauri::command(async)]
pub fn settings_values(bridge: State<'_, Bridge>) -> Value {
    let user = user_paths().and_then(|(_, read)| read_text(&read).map(|text| (read, text)));
    let workspace =
        workspace_file(&bridge).and_then(|path| read_text(&path).map(|text| (path, text)));
    let (user_path, user_text) = match &user {
        Ok((path, text)) => (Some(path.display().to_string()), text.as_str()),
        Err(_) => (None, ""),
    };
    let (workspace_path, workspace_text, workspace_exists) = match &workspace {
        Ok((path, text)) => (
            Some(path.display().to_string()),
            text.as_str(),
            path.exists(),
        ),
        Err(_) => (None, "", false),
    };
    let mut effective = Map::new();
    let mut config_error = None;
    match bridge.args().load_app() {
        Ok(app) => {
            let whole = serde_json::to_value(&app.config).unwrap_or(Value::Null);
            for setting in settings::manifest() {
                let pointer = format!("/{}", setting.key.replace('.', "/"));
                if let Some(value) = whole.pointer(&pointer) {
                    effective.insert(setting.key.clone(), value.clone());
                }
            }
        }
        Err(error) => config_error = Some(format!("{error:#}")),
    }
    json!({
        "user_path": user_path,
        "workspace_path": workspace_path,
        "workspace_exists": workspace_exists,
        "user": values_in(user_text),
        "workspace": values_in(workspace_text),
        "effective": effective,
        "config_error": config_error,
        "file_error": user.err().or(workspace.err()),
    })
}

/// Changes one setting in one file, or removes it (`value` null) so the default applies again.
#[tauri::command(async)]
pub fn settings_set(
    bridge: State<'_, Bridge>,
    scope: String,
    key: String,
    value: Option<Value>,
) -> Value {
    match change(&bridge, &scope, &key, value.as_ref()) {
        Ok(issues) => json!({"ok": true, "issues": issues}),
        Err(error) => json!({"ok": false, "error": error}),
    }
}

fn change(
    bridge: &Bridge,
    scope: &str,
    key: &str,
    value: Option<&Value>,
) -> Result<Vec<Value>, String> {
    let scope = scope_of(scope)?;
    let setting = settings::find(key).ok_or_else(|| format!("{key} is not a setting"))?;
    if scope == Scope::Workspace && setting.machine_only {
        return Err(format!(
            "{key} is a machine setting: a project's bridle.toml may not set it. Change it under User."
        ));
    }
    let (path, read_path) = match scope {
        Scope::User => user_paths()?,
        Scope::Workspace => {
            let path = workspace_file(bridge)?;
            (path.clone(), path)
        }
    };
    let existed = read_path.exists();
    let original = read_text(&read_path)?;
    let mut candidate = settings::set_value(&original, key, value)?;
    if candidate == original && path == read_path {
        return Ok(Vec::new());
    }
    if !existed && scope == Scope::User {
        candidate = format!("{MACHINE_HEADER}{candidate}");
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    }
    let before = fs::read(&path).ok();
    fs::write(&path, &candidate)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let restore = || match &before {
        Some(bytes) => {
            let _ = fs::write(&path, bytes);
        }
        None => {
            let _ = fs::remove_file(&path);
        }
    };
    match bridge.args().load_app() {
        Err(error) => {
            restore();
            Err(format!("not saved: {error:#}"))
        }
        Ok(app) => {
            let mine = |path: &str| path == key || key.starts_with(&format!("{path}."));
            if let Some(issue) = app
                .issues
                .iter()
                .find(|issue| issue.severity == Severity::Error && mine(&issue.path))
            {
                let message = format!("not saved: [{}] {}", issue.rule, issue.message);
                restore();
                // Loading again puts the in-memory configuration back to the file as it was.
                let _ = bridge.args().load_app();
                return Err(message);
            }
            Ok(app
                .issues
                .iter()
                .filter(|issue| mine(&issue.path))
                .filter_map(|issue| serde_json::to_value(issue).ok())
                .collect())
        }
    }
}
