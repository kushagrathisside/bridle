//! The window's controls for lanes (ADR-0023): list them, review one as a diff, merge some into the project, undo the last merge, remove
//! one. Each is the person's action; the model has no way to call any of them. They use the same code as `bridle lanes`.

use harness_cli::gitlayer::UndoReport;
use harness_cli::lanes;
use serde_json::{Value, json};
use tauri::State;

use crate::bridge::{Bridge, locked, workspace_of};

/// The most bytes of patch the window receives for one lane.
const PATCH_LIMIT: usize = 300_000;

fn fail(error: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": format!("{error:#}")})
}

fn main_folder(bridge: &Bridge) -> Result<std::path::PathBuf, String> {
    workspace_of(&bridge.args()).map_err(|e| format!("{e:#}"))
}

/// The lane a turn is working in now, if any: it is not touched while the turn runs.
fn busy_lane(bridge: &Bridge) -> Option<String> {
    locked(&bridge.inner)
        .active
        .as_ref()
        .and_then(|a| a.lane.clone())
}

fn applied(report: &UndoReport) -> Value {
    json!({
        "restored": report.restored,
        "removed": report.removed,
        "skipped": report.skipped.iter().map(|(path, why)| json!({"path": path, "why": why})).collect::<Vec<_>>(),
    })
}

#[tauri::command(async)]
pub fn lane_list(bridge: State<'_, Bridge>) -> Value {
    let main = match main_folder(&bridge) {
        Ok(main) => main,
        Err(error) => return fail(error),
    };
    let busy = busy_lane(&bridge);
    json!({
        "ok": true,
        "lanes": lanes::list(&main).iter().map(|lane| json!({
            "name": lane.name, "parent": lane.parent, "folder": lanes::lane_dir(&main, &lane.name).display().to_string(),
            "busy": busy.as_deref() == Some(lane.name.as_str()),
        })).collect::<Vec<_>>(),
    })
}

#[tauri::command(async)]
pub fn lane_diff(bridge: State<'_, Bridge>, name: String) -> Value {
    let main = match main_folder(&bridge) {
        Ok(main) => main,
        Err(error) => return fail(error),
    };
    let result = lanes::load(&main, &name).and_then(|lane| lanes::diff(&main, &lane));
    match result {
        Ok(diff) => {
            let mut patch = diff.patch;
            let cut = patch.len() > PATCH_LIMIT;
            if cut {
                let mut end = PATCH_LIMIT;
                while !patch.is_char_boundary(end) {
                    end -= 1;
                }
                patch.truncate(end);
                patch.push_str("\n... the rest of the patch is not shown\n");
            }
            json!({
                "ok": true,
                "files": diff.files.iter().map(|(kind, path)| json!({"status": kind.to_string(), "path": path})).collect::<Vec<_>>(),
                "stat": diff.stat, "patch": patch,
            })
        }
        Err(error) => fail(error),
    }
}

#[tauri::command(async)]
pub fn lane_integrate(
    bridge: State<'_, Bridge>,
    names: Vec<String>,
    dry_run: bool,
    skip_conflicts: bool,
) -> Value {
    if locked(&bridge.inner).running {
        return fail("a turn is running; wait for it to finish, or stop it, before merging lanes");
    }
    let main = match main_folder(&bridge) {
        Ok(main) => main,
        Err(error) => return fail(error),
    };
    match lanes::integrate(&main, &names, skip_conflicts, dry_run) {
        Ok(report) => json!({
            "ok": true,
            "dry_run": report.dry_run,
            "merged": report.merged,
            "conflicted": report.conflicted.iter().map(|(lane, files)| json!({"lane": lane, "files": files})).collect::<Vec<_>>(),
            "files": report.files.iter().map(|(kind, path)| json!({"status": kind.to_string(), "path": path})).collect::<Vec<_>>(),
            "applied": applied(&report.applied),
            "note": report.note,
            "undoable": report.after_ref.is_some(),
        }),
        Err(error) => fail(error),
    }
}

#[tauri::command(async)]
pub fn lane_undo(bridge: State<'_, Bridge>) -> Value {
    if locked(&bridge.inner).running {
        return fail(
            "a turn is running; wait for it to finish, or stop it, before undoing a merge",
        );
    }
    let main = match main_folder(&bridge) {
        Ok(main) => main,
        Err(error) => return fail(error),
    };
    match lanes::undo_last(&main) {
        Ok(report) => json!({"ok": true, "applied": applied(&report)}),
        Err(error) => fail(error),
    }
}

#[tauri::command(async)]
pub fn lane_remove(bridge: State<'_, Bridge>, name: String) -> Value {
    if busy_lane(&bridge).as_deref() == Some(name.as_str()) {
        return fail("a turn is working in this lane; stop it first");
    }
    let main = match main_folder(&bridge) {
        Ok(main) => main,
        Err(error) => return fail(error),
    };
    match lanes::remove(&main, &name) {
        Ok(()) => json!({"ok": true}),
        Err(error) => fail(error),
    }
}
