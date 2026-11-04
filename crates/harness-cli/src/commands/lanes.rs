//! `bridle lanes`: separate working copies for members who work at the same time, and the person's step of merging them (ADR-0023).
//!
//! `new` makes a lane and prints its folder; give that folder to a member as its workspace (`bridle --workspace FOLDER ...`). `diff`
//! shows what a lane changed. `integrate` merges lanes three ways into the project and writes nothing if one conflicts. `undo` puts the
//! project back as it was before the last integration. No model can run any of this.

use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;

use crate::app::App;
use crate::lanes;

#[derive(Debug, Subcommand)]
pub enum LanesAction {
    /// Make a lane: a snapshot of the project (uncommitted changes included) checked out in its own folder.
    New {
        /// Lowercase letters, digits, - and _ (at most 32).
        name: String,
        /// Start from another lane's work instead of from the project.
        #[arg(long)]
        from: Option<String>,
    },
    /// List the lanes.
    List,
    /// Print a lane's folder (to use with `--workspace`).
    Path { name: String },
    /// Show what a lane changed since it started: the files, a summary and the patch.
    Diff {
        name: String,
        /// Only the files and the summary.
        #[arg(long)]
        stat: bool,
    },
    /// Merge lanes into the project, in the order given. Nothing is written if a lane conflicts.
    Integrate {
        #[arg(required = true)]
        names: Vec<String>,
        /// Leave out a lane that conflicts and merge the rest.
        #[arg(long)]
        skip_conflicts: bool,
        /// Show what would change and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Remove the lanes that were merged.
        #[arg(long)]
        remove: bool,
    },
    /// Put the project back as it was before the last integration.
    Undo,
    /// Remove a lane (its folder, record and checkpoints).
    Remove { name: String },
}

fn show_files(files: &[(char, String)]) {
    for (kind, path) in files {
        println!("  {kind} {path}");
    }
}

pub fn run(app: &App, action: &LanesAction) -> Result<ExitCode> {
    let ws = &app.workspace;
    match action {
        LanesAction::New { name, from } => {
            let lane = lanes::new_lane(ws, name, from.as_deref())?;
            println!(
                "lane `{}` started from {}",
                lane.name,
                lane.parent
                    .as_deref()
                    .map_or("the project as it is now".to_owned(), |p| format!(
                        "lane `{p}`"
                    ))
            );
            println!("folder: {}", lanes::lane_dir(ws, name).display());
            println!(
                "give it to a member as its workspace, for example: bridle --workspace \"{}\" ...",
                lanes::lane_dir(ws, name).display()
            );
            println!("files Git ignores (build folders, .env) are not in a lane");
            Ok(ExitCode::SUCCESS)
        }
        LanesAction::List => {
            let all = lanes::list(ws);
            if all.is_empty() {
                println!("no lanes");
            }
            for lane in all {
                let from = lane
                    .parent
                    .as_deref()
                    .map_or(String::new(), |p| format!(", from `{p}`"));
                println!(
                    "{}{from}: {}",
                    lane.name,
                    lanes::lane_dir(ws, &lane.name).display()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        LanesAction::Path { name } => {
            lanes::load(ws, name)?;
            println!("{}", lanes::lane_dir(ws, name).display());
            Ok(ExitCode::SUCCESS)
        }
        LanesAction::Diff { name, stat } => {
            let lane = lanes::load(ws, name)?;
            let d = lanes::diff(ws, &lane)?;
            if d.files.is_empty() {
                println!("lane `{name}` has changed nothing");
                return Ok(ExitCode::SUCCESS);
            }
            println!("lane `{name}` changed {} file(s):", d.files.len());
            show_files(&d.files);
            println!("{}", d.stat);
            if !stat {
                println!("{}", d.patch);
            }
            Ok(ExitCode::SUCCESS)
        }
        LanesAction::Integrate {
            names,
            skip_conflicts,
            dry_run,
            remove,
        } => {
            let report = lanes::integrate(ws, names, *skip_conflicts, *dry_run)?;
            for (lane, files) in &report.conflicted {
                println!("lane `{lane}` conflicts in: {}", files.join(", "));
            }
            if let Some(note) = &report.note {
                println!("{note}");
            }
            if !report.merged.is_empty() {
                println!(
                    "{} {}: {}",
                    if report.dry_run {
                        "would merge"
                    } else {
                        "merged"
                    },
                    report.merged.len(),
                    report.merged.join(", ")
                );
                show_files(&report.files);
            }
            for path in &report.applied.restored {
                println!("  written: {path}");
            }
            for path in &report.applied.removed {
                println!("  removed: {path}");
            }
            for (path, why) in &report.applied.skipped {
                println!("  LEFT ALONE: {path} ({why})");
            }
            if let (Some(_), Some(_)) = (&report.before_ref, &report.after_ref) {
                println!(
                    "the project was checkpointed before and after; `bridle lanes undo` puts it back"
                );
            }
            if *remove && !report.dry_run && !report.merged.is_empty() {
                for lane in &report.merged {
                    lanes::remove(ws, lane)?;
                    println!("removed lane `{lane}`");
                }
            }
            let failed = (!report.conflicted.is_empty() && !*skip_conflicts)
                || !report.applied.skipped.is_empty();
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        LanesAction::Undo => {
            let report = lanes::undo_last(ws)?;
            for path in &report.restored {
                println!("  put back: {path}");
            }
            for path in &report.removed {
                println!("  removed: {path}");
            }
            for (path, why) in &report.skipped {
                println!("  LEFT ALONE: {path} ({why})");
            }
            Ok(if report.skipped.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        LanesAction::Remove { name } => {
            lanes::remove(ws, name)?;
            println!("lane `{name}` removed");
            Ok(ExitCode::SUCCESS)
        }
    }
}
