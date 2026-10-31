//! `agent tui`: the Bridle Control Center.
//!
//! A chat with the configured model slot, and an operational view of the harness around it:
//! configuration, slots, tasks, the event logs and policy decisions. It is another front end to the
//! same code the CLI and the OpenCode hook use, not a privileged one: the chat sends every tool call
//! through the hook's own policy path, there is no configuration editor, and the only other mutating
//! operations (start or stop a managed server) go through the `agent models` code after an explicit
//! confirmation.
//!
//! Layout of this module: [`state`] (state and keys, pure), [`chat`] (the chat screen's state),
//! [`ui`] (rendering), [`data`] and [`events`] (passive collection from local files), [`jobs`]
//! (background work), [`diag`] (doctor and health findings), [`watch`] (change detection for
//! auto-refresh). The agent loop itself is in [`crate::chat`].

pub mod chat;
pub mod data;
pub mod diag;
pub mod events;
pub mod jobs;
pub mod state;
pub mod ui;
pub mod watch;

use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::bail;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;

use data::LoadArgs;
use jobs::{Job, JobMsg};
use state::{Effect, Ui};

/// How long the loop waits for input before checking for finished jobs.
const TICK: Duration = Duration::from_millis(100);
/// Input events handled before the screen is drawn again, so a large paste is not drawn key by key.
const EVENTS_PER_FRAME: usize = 512;

pub fn run(args: LoadArgs) -> anyhow::Result<ExitCode> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        bail!(
            "the TUI needs an interactive terminal (stdin and stdout must both be terminals); \
             use the other subcommands for scripts"
        );
    }
    let mut terminal = ratatui::try_init()?;
    install_panic_hook();
    // A paste then arrives as one event. Not every terminal supports it (the Windows console does
    // not); `Input::more_waiting` covers those.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, &args);
    // Restore before anything is printed, on every path.
    let _ = execute!(io::stdout(), DisableBracketedPaste);
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

/// The terminal's input, with a small backlog for events that were read ahead.
#[derive(Debug, Default)]
struct Input {
    backlog: VecDeque<Event>,
}

impl Input {
    /// The next event, waiting up to `wait` for one.
    fn next(&mut self, wait: Duration) -> io::Result<Option<Event>> {
        if let Some(event) = self.backlog.pop_front() {
            return Ok(Some(event));
        }
        if event::poll(wait)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }

    /// Whether more input is already waiting. Key releases (sent by the Windows console after every
    /// press) are dropped: nothing uses them. Events that are read are kept for [`Input::next`].
    fn more_waiting(&mut self) -> io::Result<bool> {
        while event::poll(Duration::ZERO)? {
            let event = event::read()?;
            if matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Release) {
                continue;
            }
            self.backlog.push_back(event);
            return Ok(true);
        }
        Ok(!self.backlog.is_empty())
    }
}

/// How long quitting waits for a chat turn to reach a stopping point.
const QUIT_WAIT: Duration = Duration::from_secs(3);

/// Asks a running chat turn to stop and waits a moment for it, so a tool that is in the middle of a call
/// is not cut off by the process ending. A model call that has not finished is not waited for.
fn finish_chat_turn(ui: &mut Ui, rx: &mpsc::Receiver<JobMsg>) {
    if !ui.chat.running {
        return;
    }
    ui.chat
        .cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let deadline = std::time::Instant::now() + QUIT_WAIT;
    while ui.chat.running && std::time::Instant::now() < deadline {
        if let Ok(msg) = rx.recv_timeout(Duration::from_millis(100)) {
            let _ = ui.apply(msg);
        }
    }
}

fn event_loop(terminal: &mut DefaultTerminal, args: &LoadArgs) -> anyhow::Result<()> {
    let theme = ui::Theme::from_env();
    let (tx, rx) = mpsc::channel::<JobMsg>();
    let mut ui = Ui::new();
    let mut input = Input::default();
    let mut pending = ui.request(Job::Refresh);
    let mut watcher = watch::Watcher::default();
    let mut dirty = true;
    loop {
        for effect in pending.drain(..) {
            match effect {
                Effect::Quit => {
                    finish_chat_turn(&mut ui, &rx);
                    return Ok(());
                }
                Effect::Run(job) => jobs::spawn(job, args.clone(), tx.clone()),
                Effect::ChatSend => {
                    if let Some(turn) = ui.take_chat_turn() {
                        jobs::spawn_chat(turn, args.clone(), tx.clone());
                    }
                }
            }
        }
        if dirty {
            terminal.draw(|f| ui::draw(f, &ui, theme))?;
            dirty = false;
        }
        let mut wait = TICK;
        for _ in 0..EVENTS_PER_FRAME {
            let Some(event) = input.next(wait)? else {
                break;
            };
            wait = Duration::ZERO;
            match event {
                Event::Key(key) => {
                    // A terminal without bracketed paste delivers a paste as a burst of keys. An Enter
                    // with more input already waiting is a line break inside it, not "send".
                    let key = if key.kind == KeyEventKind::Press
                        && key.code == KeyCode::Enter
                        && key.modifiers.is_empty()
                        && ui.in_chat_input()
                        && input.more_waiting()?
                    {
                        KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::NONE)
                    } else {
                        key
                    };
                    pending.extend(ui.handle_key(key));
                }
                Event::Paste(text) => ui.handle_paste(&text),
                Event::Resize(..) => {}
                _ => continue,
            }
            dirty = true;
            if !pending.is_empty() {
                break;
            }
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
        // Keep the "running" indicators alive while work is in flight.
        dirty |= ui.busy.is_some() || ui.refreshing || ui.chat.running;
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
