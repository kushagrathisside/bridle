//! `agent init`: wire the harness into a repository, or record machine facts.
//!
//! Repository mode writes `local-agent.toml` (from an embedded profile, never overwritten),
//! `opencode.json` (generated), `.opencode/plugins/local-agent.ts` (the shim) and a `.gitignore`
//! entry. Generated files are tracked by hash in `.local-agent/generated.json`; a generated file
//! the user has since edited is only replaced with `--force`.
//!
//! Machine mode (`--machine`) writes the machine-level config with model/runtime directories and
//! the OpenCode binary path (directive D-2: weights live wherever the user says, e.g. `D:\models`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Args;
use harness_core::identity;
use sha2::{Digest, Sha256};

use crate::app::{App, resolve_workspace};
use crate::host::opencode_v1;

/// Embedded profiles (`configs/*.toml`).
const PROFILES: &[(&str, &str)] = &[
    (
        "local-only",
        include_str!("../../../../configs/local-only.toml"),
    ),
    (
        "development",
        include_str!("../../../../configs/development.toml"),
    ),
];

/// The OpenCode plugin shim template (`opencode-plugin/local-agent.ts`).
const SHIM_TEMPLATE: &str = include_str!("../../../../opencode-plugin/local-agent.ts");
const SHIM_BIN_PLACEHOLDER: &str = "\"__AGENT_BIN__\"";

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Profile for a new `local-agent.toml` (local-only, development).
    #[arg(long, default_value = "local-only")]
    profile: String,
    /// Replace generated files even if they were edited by hand.
    #[arg(long)]
    force: bool,
    /// Write the machine-level config instead of wiring a repository.
    #[arg(long)]
    machine: bool,
    /// Machine mode: directory holding model weights (e.g. D:\models).
    #[arg(long, requires = "machine")]
    models_dir: Option<PathBuf>,
    /// Machine mode: directory holding inference runtimes (e.g. D:\runtimes).
    #[arg(long, requires = "machine")]
    runtimes_dir: Option<PathBuf>,
    /// Machine mode: path to the opencode executable.
    #[arg(long, requires = "machine")]
    opencode_bin: Option<PathBuf>,
}

pub fn run(
    args: &InitArgs,
    config: Option<&Path>,
    workspace: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    if args.machine {
        return init_machine(args);
    }
    let ws = resolve_workspace(workspace)?;

    // 1. Repository config from a profile (never clobbered).
    let repo_cfg = ws.join(identity::CONFIG_FILE);
    if repo_cfg.exists() {
        println!("· {} exists, keeping it", identity::CONFIG_FILE);
    } else {
        let Some((_, body)) = PROFILES.iter().find(|(n, _)| *n == args.profile) else {
            let names: Vec<_> = PROFILES.iter().map(|(n, _)| *n).collect();
            bail!(
                "unknown profile `{}` (available: {})",
                args.profile,
                names.join(", ")
            );
        };
        fs::write(&repo_cfg, body).with_context(|| format!("writing {}", repo_cfg.display()))?;
        println!(
            "✓ wrote {} (profile {})",
            identity::CONFIG_FILE,
            args.profile
        );
    }

    // 2. Load the effective config (machine + repo) and generate host files from it.
    let app = App::load(config, Some(&ws))?;
    app.require_valid_config()?;
    let agent_bin = std::env::current_exe().context("locating the agent executable")?;
    let agent_bin = dunce::canonicalize(&agent_bin).unwrap_or(agent_bin);

    let oc = opencode_v1::generate_opencode_config(&app.config, &agent_bin);
    let oc_text = serde_json::to_string_pretty(&oc)? + "\n";
    let bin_literal = serde_json::to_string(&agent_bin.to_string_lossy())?;
    let shim = SHIM_TEMPLATE.replacen(SHIM_BIN_PLACEHOLDER, &bin_literal, 1);
    write_host_files(&ws, &app.state_dir(), &oc_text, &shim, args.force)?;

    // 3. Keep harness state out of git (R-16.2).
    ensure_gitignore(&ws, &[&format!("{}/", identity::STATE_DIR), ".worktrees/"])?;

    let warnings = app.issues.len();
    println!(
        "\nNext: start the main model with `{0} models serve {1} --detach`, check it with \
         `{0} health {1}`, then run `opencode` in this directory.{2}",
        identity::CLI_NAME,
        app.config.agent.default_model,
        if warnings > 0 {
            format!(" ({warnings} config warning(s): run `agent config validate`)")
        } else {
            String::new()
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// Writes the generated host files and records them in the ledger. The ledger is saved even when
/// a later file fails, so the next run recognises the files that were written.
fn write_host_files(
    ws: &Path,
    state_dir: &Path,
    oc_text: &str,
    shim: &str,
    force: bool,
) -> anyhow::Result<()> {
    let mut ledger = Ledger::load(state_dir);
    let written = ledger
        .write(ws, "opencode.json", oc_text, force)
        .and_then(|()| ledger.write(ws, ".opencode/plugins/local-agent.ts", shim, force));
    ledger.save(state_dir)?;
    written
}

/// The existing machine config, or an empty table when there is none. A file that exists but
/// cannot be read is an error: rewriting it from scratch would lose its settings.
fn read_machine_table(path: &Path) -> anyhow::Result<toml::Table> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml::Table>()
            .with_context(|| format!("parsing {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(err) => Err(err).with_context(|| {
            format!(
                "reading {}; fix or remove it before running `init --machine`",
                path.display()
            )
        }),
    }
}

fn init_machine(args: &InitArgs) -> anyhow::Result<ExitCode> {
    let path = harness_core::config::machine_config_path()
        .context("cannot determine the machine config location (set LOCAL_AGENT_HOME)")?;
    let mut table = read_machine_table(&path)?;
    let mut set = |section: &str, key: &str, value: &Option<PathBuf>| {
        if let Some(v) = value {
            let entry = table
                .entry(section.to_owned())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(t) = entry {
                t.insert(
                    key.to_owned(),
                    toml::Value::String(v.to_string_lossy().into_owned()),
                );
            }
        }
    };
    set("paths", "models_dir", &args.models_dir);
    set("paths", "runtimes_dir", &args.runtimes_dir);
    set("host", "opencode_bin", &args.opencode_bin);
    // Validate the merged result before writing so a typo cannot brick every repository.
    let body = toml::to_string_pretty(&table)?;
    harness_core::config::from_toml_str(&body).context("resulting machine config is invalid")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let header = "# Machine-level config: facts about this computer (model and runtime locations).\n\
                  # Managed by `agent init --machine`; repository settings belong in local-agent.toml.\n\n";
    fs::write(&path, format!("{header}{body}"))?;
    println!("✓ wrote {}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn ensure_gitignore(ws: &Path, entries: &[&str]) -> anyhow::Result<()> {
    let path = ws.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|e| !existing.lines().any(|l| l.trim() == *e))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("# {} harness state\n", identity::PRODUCT_ID));
    for e in &missing {
        text.push_str(e);
        text.push('\n');
    }
    fs::write(&path, text)?;
    println!("✓ .gitignore: added {}", missing.join(", "));
    Ok(())
}

/// Hashes of files this command generated, so hand edits are never silently overwritten.
#[derive(Debug, Default)]
struct Ledger {
    hashes: BTreeMap<String, String>,
}

impl Ledger {
    fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("generated.json")
    }

    fn load(state_dir: &Path) -> Self {
        let hashes = fs::read_to_string(Self::path(state_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { hashes }
    }

    fn save(&self, state_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(state_dir)?;
        fs::write(
            Self::path(state_dir),
            serde_json::to_string_pretty(&self.hashes)?,
        )?;
        Ok(())
    }

    fn write(&mut self, ws: &Path, rel: &str, content: &str, force: bool) -> anyhow::Result<()> {
        let path = ws.join(rel);
        if let Ok(current) = fs::read_to_string(&path) {
            if current == content {
                println!("· {rel} up to date");
                self.hashes.insert(rel.to_owned(), sha256_hex(content));
                return Ok(());
            }
            let ours = self
                .hashes
                .get(rel)
                .is_some_and(|h| *h == sha256_hex(&current));
            if !ours && !force {
                bail!(
                    "{rel} exists and was not generated by `{} init` (or was edited); re-run with --force to replace it",
                    identity::CLI_NAME
                );
            }
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        self.hashes.insert(rel.to_owned(), sha256_hex(content));
        println!("✓ wrote {rel}");
        Ok(())
    }
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_parse() {
        for (name, body) in PROFILES {
            harness_core::config::from_toml_str(body)
                .unwrap_or_else(|e| panic!("profile {name}: {e}"));
        }
    }

    #[test]
    fn shim_has_placeholder() {
        assert!(SHIM_TEMPLATE.contains(SHIM_BIN_PLACEHOLDER));
    }

    #[test]
    fn ledger_refuses_foreign_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("opencode.json"), "{}").expect("write");
        let mut l = Ledger::default();
        assert!(
            l.write(dir.path(), "opencode.json", "{\"a\":1}", false)
                .is_err()
        );
        assert!(
            l.write(dir.path(), "opencode.json", "{\"a\":1}", true)
                .is_ok()
        );
        // Now ours: regenerating without --force is fine.
        assert!(
            l.write(dir.path(), "opencode.json", "{\"a\":2}", false)
                .is_ok()
        );
    }

    #[test]
    fn an_unreadable_machine_config_is_an_error_not_a_blank_slate() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            read_machine_table(&dir.path().join("missing.toml"))
                .expect("absent is fine")
                .is_empty()
        );
        // A directory where the file should be cannot be read as text.
        let blocked = dir.path().join("config.toml");
        fs::create_dir(&blocked).expect("mkdir");
        assert!(read_machine_table(&blocked).is_err());
    }

    #[test]
    fn the_ledger_is_saved_when_a_later_host_file_fails() {
        let ws = tempfile::tempdir().expect("workspace");
        let state = ws.path().join(".local-agent");
        // `.opencode` is a file, so the shim under `.opencode/plugins/` cannot be written.
        fs::write(ws.path().join(".opencode"), "not a directory").expect("write");
        assert!(write_host_files(ws.path(), &state, "{}\n", "// shim", false).is_err());

        // opencode.json was written and recorded, so regenerating it needs no --force.
        let mut ledger = Ledger::load(&state);
        assert!(
            ledger
                .write(ws.path(), "opencode.json", "{\"a\":1}\n", false)
                .is_ok()
        );
    }

    #[test]
    fn gitignore_entries_added_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_gitignore(dir.path(), &[".local-agent/"]).expect("first");
        ensure_gitignore(dir.path(), &[".local-agent/"]).expect("second");
        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert_eq!(text.matches(".local-agent/").count(), 1);
    }
}
