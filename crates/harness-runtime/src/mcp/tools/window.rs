//! `window_screenshot` (ADR-0021): start a program, wait for its window, take a picture of that window, and stop the program.
//!
//! For an agent that builds a program with a window and wants to know what the window looks like. It is a **command** first: the
//! program and its arguments go through exactly the checks `run_command` makes (bare program name, plain words, the shell policy, the
//! command class, the sandbox), so a program the policy would not run is not started here either. It is offered only when
//! `[desktop] screenshots_enabled` is set in the machine configuration, and only on Windows.
//!
//! What it does not do: it never clicks or types, never looks at a window of any other program, and always stops what it started
//! (the whole process tree) before it returns. The picture is checked (a PNG, bounded), named by its hash, kept in
//! `.bridle/web/screenshots/` and recorded as a source (`extraction: "window"`). The answer is text: size, title, how it was drawn, and
//! whether the window is blank. A model that can see pictures needs the file attached by the person.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use harness_core::identity;
use serde_json::{Value, json};

use super::run::{authorize, plain_word};
use super::{McpTool, ToolContext, ToolOutput, kill_tree, redacted_output};
use crate::desktop::{self, Spec};
use crate::web::store::{SourceRecord, Sources, now};

const MAX_ARGS: usize = 16;
const DEFAULT_WAIT_MS: u64 = 10_000;
const MAX_PICTURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;

/// The tool for this configuration: none unless window screenshots are on, and none off Windows.
pub fn tools(config: &harness_core::config::Config) -> Vec<Box<dyn McpTool>> {
    if config.desktop.screenshots_enabled && cfg!(windows) {
        vec![Box::new(WindowScreenshotTool)]
    } else {
        Vec::new()
    }
}

#[derive(Debug)]
pub struct WindowScreenshotTool;

impl McpTool for WindowScreenshotTool {
    fn name(&self) -> &'static str {
        "window_screenshot"
    }
    fn description(&self) -> &'static str {
        "Start a program, wait for its window, save a picture of that window, and stop the program. For checking what a program you built \
         looks like. The program is judged by the same command policy as run_command (a bare program name on PATH, plain arguments). \
         Returns the window's size and title and whether it is blank; the picture is saved in .bridle/web/screenshots/ as a source."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {
            "program": {"type": "string", "description": "Program name on PATH, e.g. python or myapp."},
            "args": {"type": "array", "items": {"type": "string"}, "maxItems": MAX_ARGS, "description": "Arguments; letters, digits and _ . / - = : , @ + only."},
            "cwd": {"type": "string", "description": "Working directory inside the workspace (default: the workspace root)."},
            "wait_ms": {"type": "integer", "minimum": 500, "description": "How long to wait for the window to appear. Default 10000, at most the configured maximum."},
            "title_contains": {"type": "string", "description": "If the program opens several windows: text the wanted window's title contains."}
        }, "required": ["program"], "additionalProperties": false})
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        if !ctx.config.desktop.screenshots_enabled {
            return Err("window screenshots are off: set [desktop] screenshots_enabled = true in the machine configuration".to_owned());
        }
        if !cfg!(windows) {
            return Err("window screenshots are available on Windows only".to_owned());
        }
        let program = args
            .get("program")
            .and_then(Value::as_str)
            .ok_or("`program` must be a string")?;
        if !plain_word(program) || program.contains(['/', '\\', ':']) {
            return Err("`program` must be a bare program name found on PATH".to_owned());
        }
        let words: Vec<String> = match args.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or("`args` must be a list of text")
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("`args` must be a list of text".to_owned()),
        };
        if words.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments"));
        }
        if let Some(bad) = words.iter().find(|w| !plain_word(w)) {
            return Err(format!(
                "argument `{bad}` uses characters this tool does not pass"
            ));
        }
        let cwd = match args.get("cwd") {
            None | Some(Value::Null) => None,
            Some(Value::String(dir)) if !dir.is_empty() => {
                super::check_read_path(ctx, dir)?;
                Some(dir.clone())
            }
            Some(_) => return Err("`cwd` must be a non-empty string".to_owned()),
        };
        let max_wait = ctx.config.desktop.max_wait_ms;
        let wait_ms = match args.get("wait_ms") {
            None | Some(Value::Null) => DEFAULT_WAIT_MS.min(max_wait),
            Some(v) => v
                .as_u64()
                .ok_or("`wait_ms` must be a whole number of milliseconds")?
                .clamp(500, max_wait),
        };
        let title = match args.get("title_contains") {
            None | Some(Value::Null) => None,
            Some(Value::String(t))
                if t.chars().count() <= 100 && !t.chars().any(char::is_control) =>
            {
                Some(t.clone()).filter(|t| !t.is_empty())
            }
            Some(_) => {
                return Err("`title_contains` must be text of at most 100 characters".to_owned());
            }
        };

        let mut line = program.to_owned();
        for word in &words {
            line.push(' ');
            line.push_str(word);
        }
        if let Err((rule, text)) = authorize(ctx, &line, cwd.as_deref()) {
            return Ok(ToolOutput {
                text: text.clone(),
                structured: Some(
                    json!({"status": "blocked", "command": ctx.redactor.redact(&line).into_owned(), "rule": rule, "reason": text}),
                ),
                is_error: false,
            });
        }
        let started = Instant::now();
        let result = run(
            ctx,
            program,
            &words,
            cwd.as_deref(),
            Duration::from_millis(wait_ms),
            title.as_deref(),
        );
        let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        log(ctx, program, &result, millis);
        match result {
            Ok(done) => Ok(redacted_output(ctx, &done.text, Some(done.structured))),
            Err(message) => Err(ctx.redactor.redact(&message).into_owned()),
        }
    }
}

struct Done {
    text: String,
    structured: Value,
}

fn run(
    ctx: &ToolContext,
    program: &str,
    words: &[String],
    cwd: Option<&str>,
    wait: Duration,
    title: Option<&str>,
) -> Result<Done, String> {
    let dir = cwd.map_or_else(|| ctx.workspace.clone(), |c| ctx.workspace.join(c));
    let mut command = Command::new(program);
    command
        .args(words)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::hide_console(&mut command);
    let settings = crate::sandbox::Settings {
        enabled: ctx.config.sandbox.enabled,
        network: ctx.config.sandbox.network,
    };
    let _scratch = crate::sandbox::confine(&mut command, &ctx.workspace, &settings)
        .map_err(|reason| format!("refused by the sandbox: {reason}"))?;
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to start `{program}`: {e}"))?;
    let pid = child.id();
    let state = identity::state_dir(&ctx.workspace).join("tmp");
    let out = state.join(format!("window-{pid}-{}.png", std::process::id()));
    let captured = std::fs::create_dir_all(&state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))
        .and_then(|()| {
            desktop::capture(&Spec {
                pid,
                wait,
                title_contains: title.map(str::to_owned),
                out: out.clone(),
                max_pixels: MAX_PIXELS,
            })
        });
    // Whatever happened, what was started is stopped: the whole process tree.
    let exited_early = child.try_wait().ok().flatten();
    let _ = kill_tree(pid);
    let _ = child.kill();
    let _ = child.wait();
    let shot = match captured {
        Ok(shot) => shot,
        Err(message) => {
            let _ = std::fs::remove_file(&out);
            let more = exited_early.map_or(String::new(), |status| {
                format!(" (`{program}` had already exited: {status})")
            });
            return Err(format!("no picture was taken: {message}{more}"));
        }
    };
    let sources = Sources::open(&ctx.workspace);
    let kept = sources.keep_png(&out, MAX_PICTURE_BYTES);
    let _ = std::fs::remove_file(&out);
    let (path, sha, bytes) = kept?;
    let record = sources
        .record(SourceRecord {
            id: String::new(),
            url: format!("window:{program}"),
            final_url: format!("window:{program}"),
            title: shot.title.clone(),
            provider: String::new(),
            content_type: "image/png".to_owned(),
            sha256: sha.clone(),
            bytes,
            retrieved_at: now(),
            kind: "screenshot".to_owned(),
            extraction: "window".to_owned(),
            ..SourceRecord::default()
        })
        .map_err(|e| format!("cannot record the picture: {e}"))?;
    let verdict = if shot.blank() {
        "The window looks BLANK (one colour all over): it may not have drawn yet, or it cannot be captured; try a longer wait_ms."
    } else {
        "The window has content."
    };
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let text = format!(
        "Window screenshot [{}] of `{program}`: {} x {} pixels, title \"{}\", drawn by {}; about {} distinct colours. {verdict}\nSaved as .bridle/web/screenshots/{name} (sha256 {}). `{program}` was started by Bridle and stopped after the picture.\n",
        record.id,
        shot.width,
        shot.height,
        shot.title.replace('"', "'"),
        shot.method,
        shot.distinct_colours,
        &sha[..12]
    );
    Ok(Done {
        text,
        structured: json!({
            "source": record.id, "width": shot.width, "height": shot.height, "title": shot.title,
            "method": shot.method, "blank": shot.blank(), "distinct_colours": shot.distinct_colours,
            "file": format!(".bridle/web/screenshots/{name}"), "sha256": sha,
        }),
    })
}

/// One line in the tool log: which program, how long, whether a picture was taken. Not the window's title or its pixels.
fn log(ctx: &ToolContext, program: &str, result: &Result<Done, String>, millis: u64) {
    use harness_core::telemetry::{Event, EventKind};
    let t = &ctx.config.telemetry;
    let log = crate::event_log::EventLog::for_workspace(
        &ctx.workspace,
        ctx.config.security.secrets_redaction,
        t.max_field_bytes,
        t.enabled,
    );
    let mut event = Event::now(EventKind::ToolAfter);
    event.tool = Some("window_screenshot".to_owned());
    event.duration_ms = Some(millis);
    event.status = Some(if result.is_ok() { "ok" } else { "error" }.to_owned());
    event = event.with("program", program);
    if let Err(message) = result {
        event = event.with("failure", message.chars().take(300).collect::<String>());
    }
    let _ = log.append(event);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mcp::tools::test_context;
    use harness_core::config::Config;

    fn ctx(dir: &std::path::Path, enabled: bool, safe: &[&str]) -> ToolContext {
        let mut config = Config::default();
        config.desktop.screenshots_enabled = enabled;
        config.security.safe_commands = safe.iter().map(|s| (*s).to_owned()).collect();
        let mut c = test_context(dir);
        c.policy =
            harness_core::security::tool_policy::ToolPolicy::new(dir, &config.security).unwrap();
        c.config = config;
        c
    }

    #[test]
    fn the_tool_is_offered_only_when_turned_on_and_only_on_windows() {
        let off = Config::default();
        assert!(tools(&off).is_empty());
        let mut on = Config::default();
        on.desktop.screenshots_enabled = true;
        assert_eq!(tools(&on).len(), usize::from(cfg!(windows)));
    }

    #[test]
    fn arguments_are_checked_like_run_command_before_anything_is_started() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path(), true, &[]);
        let tool = WindowScreenshotTool;
        for bad in [
            json!({}),
            json!({"program": "C:\\\\Windows\\\\notepad.exe"}),
            json!({"program": "../x"}),
            json!({"program": "app", "args": ["a b"]}),
            json!({"program": "app", "args": ["$(x)"]}),
            json!({"program": "app", "args": "not a list"}),
            json!({"program": "app", "wait_ms": "soon"}),
            json!({"program": "app", "title_contains": 5}),
            json!({"program": "app", "unknown": 1}),
        ] {
            assert!(
                tool.call(&bad, &c).is_err() || bad.get("unknown").is_some(),
                "{bad}"
            );
        }
        let off = ctx(dir.path(), false, &[]);
        assert!(
            tool.call(&json!({"program": "winver"}), &off)
                .unwrap_err()
                .contains("window screenshots are off")
        );
    }

    #[test]
    fn a_program_the_policy_would_not_run_is_not_started() {
        if !cfg!(windows) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path(), true, &[]);
        // An unknown program is risky; the default policy blocks it, as it does for run_command. Nothing is started or recorded.
        let out = WindowScreenshotTool
            .call(&json!({"program": "definitely-not-a-listed-program"}), &c)
            .unwrap();
        assert!(out.text.contains("blocked by policy"), "{}", out.text);
        assert_eq!(out.structured.as_ref().unwrap()["status"], "blocked");
        assert!(Sources::open(dir.path()).list().is_empty());
        assert!(
            identity::state_dir(dir.path())
                .join("tmp")
                .read_dir()
                .map_or(true, |d| d.count() == 0)
        );
    }

    /// Opens a real window, so it only runs when asked for: `BRIDLE_WINDOW_TESTS=1`.
    #[test]
    fn a_real_window_is_captured_recorded_and_stopped() {
        if !cfg!(windows) || std::env::var_os("BRIDLE_WINDOW_TESTS").is_none() {
            eprintln!("skipped: set BRIDLE_WINDOW_TESTS=1 to open a real window");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path(), true, &["winver"]);
        let out = WindowScreenshotTool
            .call(&json!({"program": "winver", "wait_ms": 15000}), &c)
            .unwrap();
        assert!(
            out.text.contains("Window screenshot [S-0001]"),
            "{}",
            out.text
        );
        let data = out.structured.unwrap();
        assert!(
            data["width"].as_u64().unwrap() > 100 && data["height"].as_u64().unwrap() > 100,
            "{data}"
        );
        assert_eq!(data["blank"], false, "{data}");
        let file = dir.path().join(data["file"].as_str().unwrap());
        let png = std::fs::read(&file).unwrap();
        assert!(
            png.starts_with(b"\x89PNG\r\n\x1a\n") && png.len() > 2000,
            "{} bytes",
            png.len()
        );
        let record = &Sources::open(dir.path()).list()[0];
        assert_eq!(
            (record.kind.as_str(), record.extraction.as_str()),
            ("screenshot", "window")
        );
        // What was started is gone.
        std::thread::sleep(Duration::from_millis(1500));
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        let still = system
            .processes()
            .values()
            .filter(|p| {
                p.name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("winver.exe")
            })
            .count();
        assert_eq!(still, 0, "winver.exe was left running");
    }
}
