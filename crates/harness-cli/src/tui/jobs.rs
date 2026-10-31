//! Background jobs. Every operation that reads more than a few small files or can take time runs on
//! a worker thread and reports back over a channel, so the interface never blocks.
//!
//! Jobs call the same functions the CLI commands use (`doctor::gather`, `health::check_slot`,
//! `models::start_detached`, `models::stop_server`). Each job loads the configuration afresh, so
//! it runs against the validated, current config and the same trust rules as any CLI invocation.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use harness_runtime::health::{CheckResult, Status, summarize};

use crate::chat::{Agent, TurnEnd};
use crate::commands::doctor;
use crate::commands::health::check_slot;
use crate::commands::models::{self, ServerState, StopOutcome};
use crate::tui::chat::{ChatMsg, Turn};
use crate::tui::data::{LoadArgs, Snapshot};
use crate::tui::diag::{self, DiagItem};

/// Name of worker threads; the panic hook stays quiet for them (see `tui::install_panic_hook`).
pub const WORKER_THREAD: &str = "bridle-job";

/// How long `Start` waits for a model to become healthy.
const START_WAIT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Refresh,
    Doctor,
    Health { slot: Option<String>, quick: bool },
    Start { slot: String },
    Stop { slot: String },
}

impl Job {
    pub fn label(&self) -> String {
        match self {
            Self::Refresh => "refresh".to_owned(),
            Self::Doctor => "doctor".to_owned(),
            Self::Health { slot, quick } => format!(
                "{} health check ({})",
                if *quick { "quick" } else { "full" },
                slot.as_deref().unwrap_or("all slots")
            ),
            Self::Start { slot } => format!("start `{slot}`"),
            Self::Stop { slot } => format!("stop `{slot}`"),
        }
    }

    /// Whether the job reads only local files (a refresh) as opposed to doing real work.
    pub fn is_refresh(&self) -> bool {
        *self == Self::Refresh
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotHealth {
    pub status: Status,
    pub checks: Vec<CheckResult>,
    pub at: String,
}

#[derive(Debug)]
pub enum JobMsg {
    /// Progress of a chat turn.
    Chat(ChatMsg),
    Snapshot(Box<Snapshot>),
    Doctor {
        items: Vec<DiagItem>,
        blocking: bool,
        at: String,
    },
    Health {
        slot: String,
        health: SlotHealth,
    },
    /// The job is over; `Ok` carries a one-line result, `Err` the failure.
    Finished {
        job: Job,
        outcome: Result<String, String>,
    },
}

/// Runs `job` on a new thread. Results arrive on `tx`; the last message is always `Finished`.
pub fn spawn(job: Job, args: LoadArgs, tx: Sender<JobMsg>) {
    let spawned = thread::Builder::new()
        .name(WORKER_THREAD.to_owned())
        .spawn({
            let tx = tx.clone();
            let job = job.clone();
            move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| run(&job, &args, &tx)))
                    .unwrap_or_else(|_| Err("the job crashed (see the terminal log)".to_owned()));
                let _ = tx.send(JobMsg::Finished { job, outcome });
            }
        });
    if let Err(e) = spawned {
        let _ = tx.send(JobMsg::Finished {
            job,
            outcome: Err(format!("cannot start a worker thread: {e}")),
        });
    }
}

fn now() -> String {
    crate::tui::events::short_time(
        &humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string(),
    )
}

fn run(job: &Job, args: &LoadArgs, tx: &Sender<JobMsg>) -> Result<String, String> {
    if *job == Job::Refresh {
        let snap = Snapshot::collect(args);
        let _ = tx.send(JobMsg::Snapshot(Box::new(snap)));
        return Ok("refreshed".to_owned());
    }
    let app = args.load_app().map_err(|e| format!("{e:#}"))?;
    match job {
        Job::Refresh => unreachable!("handled above"),
        Job::Doctor => {
            let mut report = doctor::gather(&app);
            let blocking = doctor::is_blocking(&report);
            report.blocking_problems = blocking;
            let items = diag::from_report(&report);
            let _ = tx.send(JobMsg::Doctor {
                items,
                blocking,
                at: now(),
            });
            Ok(if blocking {
                "doctor found blocking problems".to_owned()
            } else {
                "doctor finished".to_owned()
            })
        }
        Job::Health { slot, quick } => {
            let slots: Vec<_> = app
                .config
                .models
                .iter()
                .filter(|(name, _)| slot.as_deref().is_none_or(|s| s == name.as_str()))
                .collect();
            if slots.is_empty() {
                return Err("no matching model slot is configured".to_owned());
            }
            let mut failed = 0;
            for (name, model) in slots {
                let checks = check_slot(model, *quick);
                let status = summarize(&checks);
                failed += usize::from(status == Status::Fail);
                let _ = tx.send(JobMsg::Health {
                    slot: name.clone(),
                    health: SlotHealth {
                        status,
                        checks,
                        at: now(),
                    },
                });
            }
            if failed == 0 {
                Ok("health checks finished".to_owned())
            } else {
                Err(format!("{failed} slot(s) failed their health checks"))
            }
        }
        Job::Start { slot } => {
            let started = std::time::Instant::now();
            models::start_checked(&app, slot, START_WAIT).map_err(|e| format!("{e:#}"))?;
            let pid = match models::server_state(&app, slot) {
                ServerState::Running { pid } => format!(" (pid {pid})"),
                _ => String::new(),
            };
            Ok(format!(
                "`{slot}` started{pid}, healthy after {:.1} s; log {}",
                started.elapsed().as_secs_f64(),
                models::server_log_path(&app, slot).display()
            ))
        }
        Job::Stop { slot } => models::stop_server(&app, slot)
            .map(|o| match o {
                StopOutcome::NoRecord => format!("no server recorded for `{slot}`"),
                StopOutcome::Stopped(pid) => format!("`{slot}` stopped (pid {pid})"),
                StopOutcome::Gone(pid) => {
                    format!("pid {pid} was not running; removed the stale record")
                }
                StopOutcome::Reused { pid, image } => format!(
                    "pid {pid} now belongs to `{image}`; removed the stale record without signalling it"
                ),
            })
            .map_err(|e| format!("{e:#}")),
    }
}

/// Runs one chat turn on a worker thread. The conversation (the agent) is built on the first message and
/// handed back with the last event, so the next turn continues it.
pub fn spawn_chat(turn: Turn, args: LoadArgs, tx: Sender<JobMsg>) {
    let spawned = thread::Builder::new()
        .name(WORKER_THREAD.to_owned())
        .spawn({
            let tx = tx.clone();
            move || {
                let crashed =
                    catch_unwind(AssertUnwindSafe(|| run_chat(turn, &args, &tx))).is_err();
                if crashed {
                    let _ = tx.send(JobMsg::Chat(ChatMsg::Failed(
                        "the chat crashed; the next message starts a new conversation".to_owned(),
                    )));
                }
            }
        });
    if let Err(error) = spawned {
        let _ = tx.send(JobMsg::Chat(ChatMsg::Failed(format!(
            "cannot start a worker thread: {error}"
        ))));
    }
}

fn run_chat(turn: Turn, args: &LoadArgs, tx: &Sender<JobMsg>) {
    let Turn {
        text,
        slot,
        agent,
        cancel,
    } = turn;
    let mut agent = match agent {
        Some(agent) => agent,
        None => match args
            .load_app()
            .map_err(|error| format!("{error:#}"))
            .and_then(Agent::new)
        {
            Ok(agent) => Box::new(agent),
            Err(error) => {
                let _ = tx.send(JobMsg::Chat(ChatMsg::Failed(error)));
                return;
            }
        },
    };
    // `/team <request>` runs a team (planner, workers, verifier) on the request; anything else is one turn.
    let team_request = text
        .strip_prefix("/team ")
        .map(|rest| rest.trim().to_owned());
    let request = team_request.clone().unwrap_or_else(|| text.clone());
    // Routing: with `[routing] auto_slot`, a request sent to the default slot may be routed to another. The reason is
    // shown in the transcript before the answer, so the person sees which model answers and why.
    let slot = match crate::route::for_request(agent.app(), &slot, &request) {
        Some(routed) => {
            let notice = format!("routed to {}: {}", routed.slot, routed.reason);
            let _ = tx.send(JobMsg::Chat(ChatMsg::Event(crate::chat::Event::Notice(
                notice,
            ))));
            routed.slot
        }
        None => slot,
    };
    let (provider, model) = match agent.provider(&slot) {
        Ok(pair) => pair,
        Err(error) => {
            let _ = tx.send(JobMsg::Chat(ChatMsg::Finished {
                agent: Some(agent),
                end: TurnEnd::Failed(error),
            }));
            return;
        }
    };
    let mut emit = |event: crate::chat::Event| {
        let _ = tx.send(JobMsg::Chat(ChatMsg::Event(event)));
    };
    let end = match team_request {
        Some(_) => agent.run_team(provider.as_ref(), &model, &request, &cancel, &mut emit),
        None => agent.run_turn(provider.as_ref(), &model, &text, &cancel, &mut emit),
    };
    let _ = tx.send(JobMsg::Chat(ChatMsg::Finished {
        agent: Some(agent),
        end,
    }));
}
