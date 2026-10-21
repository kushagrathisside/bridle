//! `agent health` (§44): functional checks against each configured model endpoint.
//!
//! Every slot gets the checks from [`harness_runtime::health::run_checks`] (the quick subset with
//! `--quick`). A slot whose provider cannot be built, for example because its API key variable is
//! unset, is reported as one failed check so the remaining slots still run.

use std::process::ExitCode;

use anyhow::bail;
use harness_core::config::{Config, ModelConfig};
use harness_runtime::health::{
    CheckKind, CheckOptions, CheckResult, Status, run_checks, summarize,
};
use harness_runtime::providers::{ProviderError, provider_for};
use serde::Serialize;

use crate::app::App;
use crate::output::{Mark, line};

/// One JSON object per slot (`--json`).
#[derive(Debug, Serialize)]
struct SlotReport<'a> {
    slot: &'a str,
    model: &'a str,
    endpoint: &'a str,
    quick: bool,
    status: Status,
    checks: &'a [CheckResult],
}

pub fn run(app: &App, slot: Option<&str>, quick: bool, json: bool) -> anyhow::Result<ExitCode> {
    let slots = select_slots(&app.config, slot)?;
    if slots.is_empty() {
        if !json {
            println!("· no model slots configured");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut any_failed = false;
    for (name, model) in slots {
        let checks = check_slot(model, quick);
        let status = summarize(&checks);
        any_failed |= status == Status::Fail;
        if json {
            let report = SlotReport {
                slot: name,
                model: &model.id,
                endpoint: &model.endpoint,
                quick,
                status,
                checks: &checks,
            };
            println!("{}", serde_json::to_string(&report)?);
        } else {
            print_text(name, model, quick, &checks, status);
        }
    }
    Ok(if any_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Runs the checks for one slot. Never fails: a provider that cannot be built is one failed check.
pub(crate) fn check_slot(model: &ModelConfig, quick: bool) -> Vec<CheckResult> {
    match provider_for(model, &|key| std::env::var(key).ok()) {
        Ok(provider) => run_checks(provider.as_ref(), model, CheckOptions { quick }),
        Err(err) => vec![setup_failure(&err)],
    }
}

fn setup_failure(err: &ProviderError) -> CheckResult {
    CheckResult {
        kind: CheckKind::Health,
        status: Status::Fail,
        detail: format!("cannot build provider: {err}"),
        latency_ms: None,
    }
}

fn select_slots<'a>(
    config: &'a Config,
    slot: Option<&str>,
) -> anyhow::Result<Vec<(&'a str, &'a ModelConfig)>> {
    match slot {
        None => Ok(config
            .models
            .iter()
            .map(|(name, model)| (name.as_str(), model))
            .collect()),
        Some(name) => match config.models.get_key_value(name) {
            Some((key, model)) => Ok(vec![(key.as_str(), model)]),
            None => bail!(
                "no model slot named `{name}` (configured: {})",
                configured_slots(config)
            ),
        },
    }
}

/// Comma-separated slot names for messages, or `none`.
pub(crate) fn configured_slots(config: &Config) -> String {
    if config.models.is_empty() {
        return "none".to_owned();
    }
    config.models.keys().cloned().collect::<Vec<_>>().join(", ")
}

fn print_text(
    name: &str,
    model: &ModelConfig,
    quick: bool,
    checks: &[CheckResult],
    status: Status,
) {
    let mode = if quick { "quick" } else { "full" };
    println!("{name}: {} at {} ({mode})", model.id, model.endpoint);
    for check in checks {
        line(
            mark_for(check.status),
            &wire_name(&check.kind),
            detail_text(check),
        );
    }
    line(mark_for(status), "result", wire_name(&status));
}

fn mark_for(status: Status) -> Mark {
    match status {
        Status::Pass => Mark::Ok,
        Status::Warn => Mark::Warn,
        Status::Fail => Mark::Fail,
        Status::Skip => Mark::Info,
    }
}

fn detail_text(check: &CheckResult) -> String {
    match check.latency_ms {
        Some(ms) => format!("{} ({ms} ms)", check.detail),
        None => check.detail.clone(),
    }
}

/// The serde wire name of a unit-like enum value, such as `tool_call` or `pass`.
pub(crate) fn wire_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use harness_core::config::from_toml_str;

    use super::*;

    const MODELS_JSON: &str = r#"{"object":"list","data":[{"id":"m","object":"model"}]}"#;
    const CHAT_JSON: &str = r#"{"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"READY"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#;

    fn slot_config(endpoint: &str, extra: &str) -> Config {
        from_toml_str(&format!(
            r#"
[models.m]
provider = "openai_compatible"
id = "m"
role = "engineer"
endpoint = "{endpoint}"
context = 4096
hardware = "gpu"
timeout_ms = 5000
retries = 0
{extra}
"#
        ))
        .expect("fixture is valid")
    }

    /// A port nothing listens on: bind, note the port, drop the listener.
    fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    }

    /// Answers `GET .../models` and `POST .../chat/completions` on a background thread.
    fn fake_openai_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve_once(stream);
            }
        });
        format!("http://{addr}/v1")
    }

    fn serve_once(mut stream: TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let head_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            match stream.read(&mut chunk) {
                Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n]),
                _ => return,
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while buf.len() < head_end + length {
            match stream.read(&mut chunk) {
                Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n]),
                _ => return,
            }
        }
        let body = if head.starts_with("POST ") {
            CHAT_JSON
        } else {
            MODELS_JSON
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    }

    #[test]
    fn unreachable_endpoint_is_a_failed_health_check() {
        let config = slot_config(&format!("http://127.0.0.1:{}/v1", closed_port()), "");
        let checks = check_slot(&config.models["m"], true);
        assert_eq!(checks[0].kind, CheckKind::Health);
        assert_eq!(checks[0].status, Status::Fail);
        assert_eq!(summarize(&checks), Status::Fail);
    }

    #[test]
    fn unset_api_key_fails_without_contacting_the_endpoint() {
        let config = slot_config(
            "http://127.0.0.1:9/v1",
            r#"api_key_env = "LOCAL_AGENT_CLI_TEST_UNSET_KEY""#,
        );
        let checks = check_slot(&config.models["m"], true);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, Status::Fail);
        assert!(checks[0].detail.contains("LOCAL_AGENT_CLI_TEST_UNSET_KEY"));
    }

    #[test]
    fn fake_server_passes_quick_checks() {
        let config = slot_config(&fake_openai_server(), "");
        let checks = check_slot(&config.models["m"], true);
        let kinds: Vec<CheckKind> = checks.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [CheckKind::Health, CheckKind::Models, CheckKind::Completion]
        );
        assert_eq!(summarize(&checks), Status::Pass, "{checks:?}");
    }

    #[test]
    fn unknown_slot_names_the_configured_ones() {
        let config = slot_config("http://127.0.0.1:9/v1", "");
        let err = select_slots(&config, Some("nope")).expect_err("unknown slot");
        assert!(err.to_string().contains("configured: m"), "{err}");
        assert_eq!(select_slots(&config, None).expect("all").len(), 1);
    }

    #[test]
    fn wire_names_are_snake_case() {
        assert_eq!(wire_name(&CheckKind::ToolCall), "tool_call");
        assert_eq!(wire_name(&Status::Pass), "pass");
    }
}
