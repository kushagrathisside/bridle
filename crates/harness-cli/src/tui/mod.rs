//! `agent tui`: the Bridle Control Center.
//!
//! An operational view of the harness: configuration, model slots, the event logs and policy
//! decisions. It is another front end to the same code the CLI uses, not a privileged one: it has no
//! command executor, no configuration editor, and its only mutating operations (start or stop a
//! managed model server) go through the `agent models` code after an explicit confirmation.
//!
//! Layout of this module: [`state`] (state and keys, pure), [`ui`] (rendering), [`data`] and
//! [`events`] (passive collection from local files), [`jobs`] (background work), [`diag`]
//! (doctor and health findings).

pub mod data;
pub mod diag;
pub mod events;
pub mod jobs;
pub mod state;
pub mod ui;
pub mod watch;

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::bail;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use data::LoadArgs;
use jobs::{Job, JobMsg};
use state::{Effect, Ui};

/// How long the loop waits for input before checking for finished jobs.
const TICK: Duration = Duration::from_millis(100);

pub fn run(args: LoadArgs) -> anyhow::Result<ExitCode> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        bail!(
            "the TUI needs an interactive terminal (stdin and stdout must both be terminals); \
             use the other subcommands for scripts"
        );
    }
    let mut terminal = ratatui::try_init()?;
    install_panic_hook();
    let result = event_loop(&mut terminal, &args);
    // Restore before anything is printed, on every path.
    ratatui::restore();
    result.map(|()| ExitCode::SUCCESS)
}

/// `ratatui::init` restores the terminal on a panic in any thread, which would tear the screen down
/// when a background job panics (that panic is caught and reported inside the UI). Panics on
/// worker threads are therefore not forwarded to the restoring hook.
fn install_panic_hook() {
    let restoring = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some(jobs::WORKER_THREAD) {
            restoring(info);
        }
    }));
}

fn event_loop(terminal: &mut DefaultTerminal, args: &LoadArgs) -> anyhow::Result<()> {
    let theme = ui::Theme::from_env();
    let (tx, rx) = mpsc::channel::<JobMsg>();
    let mut ui = Ui::new();
    let mut pending = ui.request(Job::Refresh);
    let mut watcher = watch::Watcher::default();
    let mut dirty = true;
    loop {
        for effect in pending.drain(..) {
            match effect {
                Effect::Quit => return Ok(()),
                Effect::Run(job) => jobs::spawn(job, args.clone(), tx.clone()),
            }
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &ui, theme))?;
            dirty = false;
        }
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) => pending.extend(ui.handle_key(key)),
                Event::Resize(..) => {}
                _ => continue,
            }
            dirty = true;
        }
        while let Ok(msg) = rx.try_recv() {
            pending.extend(ui.apply(msg));
            dirty = true;
        }
        // Auto-refresh: a stat of a few files every couple of seconds, a re-read only on change.
        if ui.auto && ui.loaded && !ui.refreshing && watcher.changed(args, &ui.snap) {
            pending.extend(ui.request(Job::Refresh));
            dirty = true;
        }
        // Keep the "running" line alive while a job is in flight.
        dirty |= ui.busy.is_some() || ui.refreshing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn refuses_to_run_without_a_terminal() {
        // `cargo test` captures stdout, so it is not a terminal here.
        if io::stdout().is_terminal() && io::stdin().is_terminal() {
            return;
        }
        let err = run(LoadArgs::default()).unwrap_err();
        assert!(err.to_string().contains("interactive terminal"));
    }
}
