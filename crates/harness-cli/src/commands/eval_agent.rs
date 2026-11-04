//! `bridle eval-agent`: runs a **real model** through the chat loop on scripted scenarios and scores what it did. For evaluating
//! Bridle, not for daily use (like `bench` and `eval-decisions`).
//!
//! * `notices`: does the model use a background command and read the notice that says it ended; does it re-read a file after a notice
//!   that someone else changed it (ADR-0023)?
//! * `plan`: the baseline for the project manager (the proposal in `docs/spec/lessons-from-infinity-and-harness2.md`): given an issue to
//!   split, how often does the plan have an engineering issue and a QA issue that waits for it?
//! * `lanes`: two models' turns at the same time, each in its own lane, merged by `lanes integrate`.
//!
//! Every scenario runs in a fresh temporary folder (a Git repository), through the same agent loop, tools and policy as the window. Each
//! turn has a time limit. The model server must already be running (`bridle models serve main --detach`).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use harness_runtime::mcp::McpTool;
use harness_runtime::providers::Role as MsgRole;
use serde_json::{Value, json};

use crate::app::App;
use crate::board::{BoardStore, Kind, NewIssue};
use crate::board_tools;
use crate::chat::{Agent, Event, TurnEnd};
use crate::crew::Roster;
use crate::lanes;

#[derive(Debug, Subcommand)]
pub enum EvalAgentAction {
    /// Background commands and notices: does the model use them and act on them?
    Notices {
        #[arg(long, default_value_t = 5)]
        runs: usize,
        #[arg(long, default_value = "main")]
        slot: String,
        #[arg(long, default_value_t = 240)]
        max_seconds: u64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// The project manager's plan for a split request: engineering and QA issues, QA waiting for engineering.
    Plan {
        #[arg(long, default_value_t = 10)]
        runs: usize,
        #[arg(long, default_value = "main")]
        slot: String,
        #[arg(long, default_value_t = 300)]
        max_seconds: u64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Two turns at once, each in its own lane, then `lanes integrate`.
    Lanes {
        #[arg(long, default_value = "main")]
        slot: String,
        #[arg(long, default_value_t = 600)]
        max_seconds: u64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
struct ToolRecord {
    id: String,
    name: String,
    summary: String,
    ok: bool,
    blocked: bool,
    /// The start of the tool's answer (what a failure said).
    preview: String,
}

#[derive(Debug)]
struct Outcome {
    end: String,
    seconds: f64,
    tools: Vec<ToolRecord>,
    final_text: String,
    /// Tool results (by tool call id) that carried a notice block, in order.
    noticed: Vec<String>,
    /// Names of the tools called, in order.
    sequence: Vec<String>,
}

impl Outcome {
    fn called(&self, name: &str) -> usize {
        self.sequence.iter().filter(|n| *n == name).count()
    }

    fn json(&self) -> Value {
        json!({
            "end": self.end, "seconds": (self.seconds * 10.0).round() / 10.0, "sequence": self.sequence,
            "calls": self.tools.iter().map(|t| format!("{} {}{}", t.name, t.summary.chars().take(80).collect::<String>(), if t.blocked { " [blocked]".to_owned() } else if !t.ok { format!(" [failed: {}]", t.preview.chars().take(140).collect::<String>().replace('\n', " ")) } else { String::new() })).collect::<Vec<_>>(),
            "noticed_after": self.noticed.iter().filter_map(|id| self.tools.iter().find(|t| &t.id == id).map(|t| t.name.clone())).collect::<Vec<_>>(),
            "final_text": self.final_text.chars().take(600).collect::<String>(),
        })
    }
}

fn temp_dir(label: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "bridle-eval-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "eval")
        .env("GIT_AUTHOR_EMAIL", "eval@example.com")
        .env("GIT_COMMITTER_NAME", "eval")
        .env("GIT_COMMITTER_EMAIL", "eval@example.com")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("run git")?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

/// A fresh repository with the given files committed.
fn repo(label: &str, files: &[(&str, &str)]) -> Result<PathBuf> {
    let dir = temp_dir(label)?;
    git(&dir, &["init", "-q"])?;
    git(&dir, &["config", "commit.gpgsign", "false"])?;
    std::fs::write(dir.join(".gitignore"), ".bridle/\n")?;
    for (name, text) in files {
        std::fs::write(dir.join(name), text)?;
    }
    git(&dir, &["add", "-A"])?;
    git(&dir, &["commit", "-q", "-m", "init"])?;
    Ok(dir)
}

type Extra = Option<(String, Vec<Box<dyn McpTool>>)>;

/// One turn of a real model in `workspace`, with a time limit. `watch` sees every event as it happens.
fn turn(
    config: Option<&Path>,
    workspace: &Path,
    slot: &str,
    member: Extra,
    prompt: &str,
    max_seconds: u64,
    mut watch: impl FnMut(&Event),
) -> Result<Outcome> {
    let app = App::load(config, Some(workspace))?;
    app.require_valid_config()?;
    let mut agent = match member {
        Some((persona, tools)) => Agent::for_member(app, persona, tools),
        None => Agent::new(app),
    }
    .map_err(anyhow::Error::msg)?;
    let (provider, model) = agent.provider(slot).map_err(anyhow::Error::msg)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let guard = {
        let (cancel, done) = (Arc::clone(&cancel), Arc::clone(&done));
        std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(max_seconds);
            while !done.load(Ordering::Relaxed) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(200));
            }
            if !done.load(Ordering::Relaxed) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
    };
    let started = Instant::now();
    let mut tools: Vec<ToolRecord> = Vec::new();
    let end = {
        let mut emit = |event: Event| {
            match &event {
                Event::ToolStart { id, name, summary } => tools.push(ToolRecord {
                    id: id.clone(),
                    name: name.clone(),
                    summary: summary.clone(),
                    ok: true,
                    blocked: false,
                    preview: String::new(),
                }),
                Event::ToolDone {
                    id, ok, preview, ..
                } => {
                    if let Some(t) = tools.iter_mut().find(|t| &t.id == id) {
                        t.ok = *ok;
                        t.preview = preview.clone();
                    }
                }
                Event::ToolBlocked {
                    id, name, summary, ..
                } => {
                    if let Some(t) = tools.iter_mut().find(|t| &t.id == id) {
                        t.blocked = true;
                        t.ok = false;
                    } else {
                        tools.push(ToolRecord {
                            id: id.clone(),
                            name: name.clone(),
                            summary: summary.clone(),
                            ok: false,
                            blocked: true,
                            preview: String::new(),
                        });
                    }
                }
                _ => {}
            }
            watch(&event);
        };
        agent.run_turn(provider.as_ref(), &model, prompt, &cancel, &mut emit)
    };
    done.store(true, Ordering::Relaxed);
    let _ = guard.join();
    let history = agent.history();
    let mut sequence = Vec::new();
    for message in history {
        for call in &message.tool_calls {
            sequence.push(call.name.clone());
        }
    }
    let noticed = history
        .iter()
        .filter(|m| {
            m.role == MsgRole::Tool
                && m.content
                    .as_deref()
                    .is_some_and(|c| c.contains("[[bridle notices "))
        })
        .filter_map(|m| m.tool_call_id.clone())
        .collect();
    Ok(Outcome {
        end: match &end {
            TurnEnd::Done => "done".to_owned(),
            TurnEnd::Stopped => "stopped (time limit)".to_owned(),
            TurnEnd::StepLimit => "step limit".to_owned(),
            TurnEnd::Failed(why) => format!("failed: {why}"),
        },
        seconds: started.elapsed().as_secs_f64(),
        tools,
        final_text: agent.last_answer().unwrap_or_default(),
        noticed,
        sequence,
    })
}

fn rate(passed: usize, total: usize) -> String {
    format!("{passed}/{total}")
}

fn write_out(out: Option<&Path>, value: &Value) -> Result<()> {
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(value)?)
            .with_context(|| format!("write {}", path.display()))?;
        println!("results written to {}", path.display());
    }
    Ok(())
}

pub fn run(action: &EvalAgentAction, config: Option<&Path>) -> Result<ExitCode> {
    match action {
        EvalAgentAction::Notices {
            runs,
            slot,
            max_seconds,
            out,
        } => notices(config, *runs, slot, *max_seconds, out.as_deref()),
        EvalAgentAction::Plan {
            runs,
            slot,
            max_seconds,
            out,
        } => plan(config, *runs, slot, *max_seconds, out.as_deref()),
        EvalAgentAction::Lanes {
            slot,
            max_seconds,
            out,
        } => lanes_scenario(config, slot, *max_seconds, out.as_deref()),
    }
}

// ------------------------------------------------------------------------------------------------------------- notices

/// A command that takes a few seconds, and the words any one of which shows the answer reports its outcome.
///
/// Inside Bridle's sandbox a ping to this machine can time out (exit code 1), so the answer is not scored on what ping printed: the model
/// must report the outcome in words that fit either result.
fn slow_command() -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
) {
    if cfg!(windows) {
        (
            "ping",
            &["-n", "4", "127.0.0.1"],
            &[
                "average",
                "timed out",
                "loss",
                "exit",
                "failed",
                "packets",
                "reply",
                "replies",
                "round trip",
            ],
        )
    } else {
        ("git", &["--version"], &["git version"])
    }
}

fn notices(
    config: Option<&Path>,
    runs: usize,
    slot: &str,
    max_seconds: u64,
    out: Option<&Path>,
) -> Result<ExitCode> {
    let (program, args, tokens) = slow_command();
    let reports = |text: &str| {
        let lower = text.to_lowercase();
        tokens.iter().any(|t| lower.contains(t))
    };
    let command = format!("{program} {}", args.join(" "));
    let mut all = Vec::new();
    let (
        mut b1_pass,
        mut b2_pass,
        mut b2_noticed,
        mut b2_reacted,
        mut n1_pass,
        mut n1_reread,
        mut n1_refused,
    ) = (0, 0, 0, 0, 0, 0, 0);
    let (mut b2_polls, mut n0_pass, mut n0_refused) = (0_usize, 0, 0);

    for n in 1..=runs {
        // B1: told exactly what to do with the tools.
        let ws = repo("b1", &[("README.txt", "hello\n"), ("notes.txt", "alpha\n")])?;
        let prompt = format!(
            "Do these in order. 1) Start `{command}` as a background command with the command_start tool. 2) While it runs, list the project files with list_files. 3) Then call command_status with its id and tell me the last line it printed."
        );
        let o = turn(config, &ws, slot, None, &prompt, max_seconds, |_| {})?;
        let pass = o.called("command_start") >= 1
            && o.called("command_status") >= 1
            && reports(&o.final_text);
        b1_pass += usize::from(pass);
        println!(
            "run {n} B1 (told what to do): {} - {} ({:.0} s) sequence {:?}",
            if pass { "PASS" } else { "FAIL" },
            o.end,
            o.seconds,
            o.sequence
        );
        all.push(json!({"run": n, "scenario": "B1", "pass": pass, "outcome": o.json()}));

        // B2: told only to start it, to go on, and that Bridle will say when it has finished.
        let ws = repo("b2", &[("README.txt", "hello\n"), ("notes.txt", "alpha\n")])?;
        let prompt = format!(
            "Start `{command}` in the background with command_start. While it runs, list the project files with list_files and read README.txt with read_file. Bridle will tell you when the background command has finished; when it does, read its output and tell me the result in one sentence. Do not finish before you have the result."
        );
        let o = turn(config, &ws, slot, None, &prompt, max_seconds, |_| {})?;
        let started = o.called("command_start") >= 1;
        let noticed = !o.noticed.is_empty();
        // It reacted when a command_status call came after the first result that carried a notice.
        let first_notice = o.tools.iter().position(|t| o.noticed.contains(&t.id));
        let reacted = first_notice.is_some_and(|i| {
            o.tools
                .iter()
                .skip(i + 1)
                .any(|t| t.name == "command_status")
        }) || (noticed && o.called("command_status") >= 1);
        let pass = started && o.called("command_status") >= 1 && reports(&o.final_text);
        b2_pass += usize::from(pass);
        b2_polls += o.called("command_status");
        b2_noticed += usize::from(noticed);
        b2_reacted += usize::from(reacted);
        println!(
            "run {n} B2 (only told it will be announced): {} - noticed {noticed}, read the result after the notice {reacted} - {} ({:.0} s) sequence {:?}",
            if pass { "PASS" } else { "FAIL" },
            o.end,
            o.seconds,
            o.sequence
        );
        all.push(json!({"run": n, "scenario": "B2", "pass": pass, "noticed": noticed, "reacted": reacted, "outcome": o.json()}));

        // N0: the same task with nobody changing the file: how often does an edit fail anyway?
        let ws = repo("n0", &[("notes.txt", "alpha\nbravo\ncharlie\n")])?;
        let prompt = "First read notes.txt. Then run git_status. Then, in notes.txt, change the word bravo to BRAVO with edit_file. Then tell me the final content of notes.txt.";
        let o = turn(config, &ws, slot, None, prompt, max_seconds, |_| {})?;
        let text = std::fs::read_to_string(ws.join("notes.txt"))
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let kept = text.contains("BRAVO") && !text.contains("bravo\n");
        let refused = o
            .tools
            .iter()
            .filter(|t| t.name == "edit_file" && !t.ok)
            .count();
        n0_pass += usize::from(kept);
        n0_refused += usize::from(refused > 0);
        println!(
            "run {n} N0 (control, nobody changes the file): {} - edit refused {refused}x - {} ({:.0} s) calls {:?}",
            if kept { "PASS" } else { "FAIL" },
            o.end,
            o.seconds,
            o.json()["calls"]
        );
        all.push(json!({"run": n, "scenario": "N0", "pass": kept, "edit_refusals": refused, "outcome": o.json()}));

        // N1: someone else changes a file after the model read it.
        let ws = repo("n1", &[("notes.txt", "alpha\nbravo\ncharlie\n")])?;
        let notes = ws.join("notes.txt");
        let mut changed = false;
        let prompt = "First read notes.txt. Then run git_status. Then, in notes.txt, change the word bravo to BRAVO with edit_file. Then tell me the final content of notes.txt.";
        let o = turn(config, &ws, slot, None, prompt, max_seconds, |event| {
            // The person edits the file in their editor right after the model has read it.
            if let Event::ToolDone { name, ok: true, .. } = event
                && name == "read_file"
                && !changed
            {
                changed = true;
                let _ = std::fs::write(
                    &notes,
                    "alpha\nbravo\ncharlie\ndelta, added by the person\n",
                );
            }
        })?;
        let text = std::fs::read_to_string(&notes)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let kept = text.contains("delta, added by the person")
            && text.contains("BRAVO")
            && !text.contains("bravo\n");
        let refused = o
            .tools
            .iter()
            .filter(|t| t.name == "edit_file" && !t.ok)
            .count();
        let reread = o.tools.iter().filter(|t| t.name == "read_file").count() >= 2;
        let noticed = !o.noticed.is_empty();
        n1_pass += usize::from(kept);
        n1_reread += usize::from(reread);
        n1_refused += usize::from(refused > 0);
        println!(
            "run {n} N1 (file changed under it): {} - notice seen {noticed}, re-read {reread}, edit refused {refused}x - {} ({:.0} s) sequence {:?}",
            if kept { "PASS" } else { "FAIL" },
            o.end,
            o.seconds,
            o.sequence
        );
        all.push(json!({"run": n, "scenario": "N1", "pass": kept, "noticed": noticed, "reread": reread, "edit_refusals": refused, "outcome": o.json()}));
    }
    println!(
        "\nsummary over {runs} run(s) each, model slot `{slot}`:\n  B1 told to use command_start and command_status: passed {}\n  B2 only told it will be announced: passed {}, saw a notice {}, read the result after a notice {}, {} command_status call(s) in all\n  N0 control, nobody changes the file: edit done {}, had an edit refused {}\n  N1 file changed after it was read: kept both edits {}, re-read the file {}, had an edit refused {}",
        rate(b1_pass, runs),
        rate(b2_pass, runs),
        rate(b2_noticed, runs),
        rate(b2_reacted, runs),
        b2_polls,
        rate(n0_pass, runs),
        rate(n0_refused, runs),
        rate(n1_pass, runs),
        rate(n1_reread, runs),
        rate(n1_refused, runs)
    );
    write_out(out, &json!({"slot": slot, "runs": runs, "results": all}))?;
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------------------------------------------- plan

fn plan(
    config: Option<&Path>,
    runs: usize,
    slot: &str,
    max_seconds: u64,
    out: Option<&Path>,
) -> Result<ExitCode> {
    let roster = Roster::default();
    let mut all = Vec::new();
    let (mut made_children, mut roles_ok, mut waits_ok, mut full) = (0, 0, 0, 0);
    for n in 1..=runs {
        let ws = repo("plan", &[("calc.py", "def add(a, b):\n    return a + b\n")])?;
        let store = BoardStore::open(&ws);
        let (parent, _) = store.create(
            "user",
            &NewIssue {
                title: "Add sub(a, b) to calc.py".to_owned(),
                body: "calc.py has add(a, b). Add sub(a, b), which returns a - b. Split the work into one issue for engineering (write the function) and one issue for QA (check it) that waits for the engineering issue.".to_owned(),
                kind: Some(Kind::Task),
                assignee: Some("priya".to_owned()),
                ..NewIssue::default()
            },
        )?;
        let run = format!("run-{}", parent.id);
        let claimed = store.claim(&parent.id, "priya", Some(&run))?;
        let persona = roster
            .persona("priya")
            .context("the default roster has no priya")?;
        let tools = board_tools::member_tools(store.clone(), roster.clone(), "priya", &run);
        let o = turn(
            config,
            &ws,
            slot,
            Some((format!("{persona}\n\n{}", board_tools::GUIDE), tools)),
            &board_tools::brief(&claimed),
            max_seconds,
            |_| {},
        )?;
        let board = store.load()?;
        let children: Vec<_> = board
            .issues
            .values()
            .filter(|i| i.parent.as_deref() == Some(parent.id.as_str()))
            .collect();
        let team_of = |i: &&crate::board::Issue| -> String {
            i.assignee
                .as_deref()
                .and_then(|a| roster.member(a))
                .map(|m| m.role.clone())
                .unwrap_or_default()
        };
        let eng: Vec<_> = children
            .iter()
            .filter(|i| team_of(i).starts_with("eng."))
            .collect();
        let qa: Vec<_> = children
            .iter()
            .filter(|i| team_of(i).starts_with("qa."))
            .collect();
        let has_children = children.len() >= 2;
        let roles = !eng.is_empty() && !qa.is_empty();
        let waits = qa
            .iter()
            .any(|q| eng.iter().any(|e| q.blocked_by.contains(&e.id)));
        made_children += usize::from(has_children);
        roles_ok += usize::from(roles);
        waits_ok += usize::from(waits);
        let pass = has_children && roles && waits;
        full += usize::from(pass);
        println!(
            "run {n}: {} - {} child issue(s), engineering {} / QA {}, QA waits for engineering {waits} - {} ({:.0} s) sequence {:?}",
            if pass { "PASS" } else { "FAIL" },
            children.len(),
            eng.len(),
            qa.len(),
            o.end,
            o.seconds,
            o.sequence
        );
        all.push(json!({
            "run": n, "pass": pass, "children": children.len(), "engineering": eng.len(), "qa": qa.len(), "qa_waits": waits,
            "titles": children.iter().map(|c| c.title.clone()).collect::<Vec<_>>(), "outcome": o.json()
        }));
    }
    println!(
        "\nplanner baseline over {runs} run(s), model slot `{slot}`:\n  two or more child issues: {}\n  an engineering and a QA issue: {}\n  the QA issue waits for the engineering issue: {}\n  all three: {}",
        rate(made_children, runs),
        rate(roles_ok, runs),
        rate(waits_ok, runs),
        rate(full, runs)
    );
    write_out(out, &json!({"slot": slot, "runs": runs, "results": all}))?;
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------------------------------------------- lanes

fn lanes_scenario(
    config: Option<&Path>,
    slot: &str,
    max_seconds: u64,
    out: Option<&Path>,
) -> Result<ExitCode> {
    let ws = repo(
        "lanes",
        &[
            ("calc.py", "def add(a, b):\n    return a + b\n"),
            ("README.md", "# Calc\n"),
        ],
    )?;
    lanes::new_lane(&ws, "calc", None)?;
    lanes::new_lane(&ws, "docs", None)?;
    let calc_dir = lanes::lane_dir(&ws, "calc");
    let docs_dir = lanes::lane_dir(&ws, "docs");
    let started = Instant::now();
    // Two turns at the same time, each in its own lane. (One model server answers them in turn; the turns themselves overlap.)
    let (a, b) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            turn(
                config,
                &calc_dir,
                slot,
                None,
                "In calc.py add a function sub(a, b) that returns a - b, using edit_file. Then say done.",
                max_seconds,
                |_| {},
            )
        });
        let second = scope.spawn(|| {
            turn(
                config,
                &docs_dir,
                slot,
                None,
                "Add the line `Supports add and sub.` at the end of README.md, using edit_file. Then say done.",
                max_seconds,
                |_| {},
            )
        });
        (first.join(), second.join())
    });
    let a = a.map_err(|_| anyhow::anyhow!("the calc turn crashed"))??;
    let b = b.map_err(|_| anyhow::anyhow!("the docs turn crashed"))??;
    println!(
        "both turns together took {:.0} s",
        started.elapsed().as_secs_f64()
    );
    println!(
        "lane calc: {} ({:.0} s) sequence {:?}",
        a.end, a.seconds, a.sequence
    );
    println!(
        "lane docs: {} ({:.0} s) sequence {:?}",
        b.end, b.seconds, b.sequence
    );
    let calc_diff = lanes::diff(&ws, &lanes::load(&ws, "calc")?)?;
    let docs_diff = lanes::diff(&ws, &lanes::load(&ws, "docs")?)?;
    println!(
        "calc changed {:?}; docs changed {:?}",
        calc_diff.files, docs_diff.files
    );
    let report = lanes::integrate(&ws, &["calc".to_owned(), "docs".to_owned()], false, false)?;
    let calc = std::fs::read_to_string(ws.join("calc.py"))
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let readme = std::fs::read_to_string(ws.join("README.md"))
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let calc_ok = calc.contains("def sub(") && calc.contains("a - b") && calc.contains("def add(");
    let docs_ok = readme.contains("Supports add and sub.");
    let isolated = calc_diff.files.iter().all(|(_, p)| p == "calc.py")
        && docs_diff.files.iter().all(|(_, p)| p == "README.md");
    println!(
        "integrate: merged {:?}, conflicts {:?}\nproject now: calc.py has sub {calc_ok}, README has the line {docs_ok}; each lane touched only its own file {isolated}",
        report.merged, report.conflicted
    );
    let pass = calc_ok && docs_ok && isolated && report.conflicted.is_empty();
    println!("{}", if pass { "PASS" } else { "FAIL" });
    write_out(
        out,
        &json!({"slot": slot, "pass": pass, "calc": a.json(), "docs": b.json(), "calc_files": calc_diff.files.iter().map(|(k, p)| format!("{k} {p}")).collect::<Vec<_>>(), "docs_files": docs_diff.files.iter().map(|(k, p)| format!("{k} {p}")).collect::<Vec<_>>()}),
    )?;
    Ok(if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
