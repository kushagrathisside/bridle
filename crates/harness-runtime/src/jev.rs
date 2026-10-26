//! The Jev decision engine (M7): asks the configured Jev service about a stalled loop over HTTPS, or over
//! loopback HTTP for a local gateway. Every other case, and every failure, is decided by the rules.
//!
//! The request carries the bounded summary the micro model gets: the phase, the counters, the last action and the
//! names of the loop signals. No prompt, command text or tool output leaves the machine. The key is read once, from
//! the environment variable the config names, and is sent only in the `Authorization` header. It never appears in a
//! decision, an error or a debug print.

use std::time::{Duration, Instant};

use harness_core::config::JevSection;
use harness_core::decision::{DecisionEngine, DecisionInput, DecisionResult, RulesDecisionEngine};
use harness_core::jev::{self, DECISION_PATH, JevChoice, SCHEMA};
use harness_core::lfm::{CHOICES, is_stalled};
use serde_json::{Value, json};

use crate::decision::signal_name;
use crate::providers::openai_compat::{read_text, transport_error};

/// The name recorded for decisions made by Jev.
pub const PROVIDER_NAME: &str = "jev";
/// The longest action text sent to the service.
const MAX_ACTION: usize = 120;

/// A decision engine that asks the Jev service for stalled loops.
pub struct JevEngine {
    url: String,
    api_key: Option<String>,
    timeout: Duration,
    agent: ureq::Agent,
}

impl std::fmt::Debug for JevEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is deliberately left out.
        f.debug_struct("JevEngine").field("url", &self.url).finish()
    }
}

impl JevEngine {
    /// Builds the engine from `[external.jev]`. A key the config names must be set in the environment; a missing
    /// key is an error here, before any call, not an anonymous request.
    pub fn from_config(
        section: &JevSection,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let endpoint = section
            .endpoint
            .as_deref()
            .filter(|e| !e.trim().is_empty())
            .ok_or_else(|| "no endpoint is set".to_owned())?;
        let api_key = match section.api_key_env.as_deref() {
            None => None,
            Some(name) => Some(
                env(name)
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| format!("environment variable {name} is not set"))?,
            ),
        };
        let timeout = Duration::from_millis(section.timeout_ms);
        Ok(Self {
            url: format!("{}{DECISION_PATH}", endpoint.trim_end_matches('/')),
            api_key,
            timeout,
            agent: crate::http::remote_agent(timeout),
        })
    }

    /// The request body for a stalled loop: the contract name, a bounded state, the signal names and the choices.
    pub fn request(input: &DecisionInput<'_>) -> Value {
        let state = input.state;
        let action: String = state
            .last_action
            .as_deref()
            .unwrap_or("none")
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_ACTION)
            .collect();
        let signals: Vec<&str> = input.signals.iter().map(signal_name).collect();
        json!({
            "schema": SCHEMA,
            "question": "stalled_loop",
            "state": {
                "phase": state.phase.name(),
                "iteration": state.iteration,
                "failure_count": state.failure_count,
                "consecutive_refusals": state.consecutive_refusals,
                "last_action": action,
            },
            "signals": signals,
            "choices": CHOICES,
        })
    }

    fn ask(&self, input: &DecisionInput<'_>) -> Result<JevChoice, String> {
        let mut request = self.agent.post(self.url.as_str());
        if let Some(key) = &self.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(Self::request(input))
            .map_err(|error| transport_error(error, self.timeout).to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        let text = read_text(response.body_mut(), self.timeout).map_err(|e| e.to_string())?;
        jev::parse_decision(&text).map_err(|reason| reason.to_string())
    }
}

impl DecisionEngine for JevEngine {
    fn provider(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn decide(&self, input: &DecisionInput<'_>) -> DecisionResult {
        let rules = RulesDecisionEngine.decide(input);
        if !is_stalled(rules.reason_code) {
            return rules;
        }
        let started = Instant::now();
        let outcome = self.ask(input);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match outcome {
            Ok(choice) => DecisionResult {
                decision: choice.decision,
                confidence: Some(choice.confidence),
                reason_code: rules.reason_code,
                provider: PROVIDER_NAME.to_owned(),
                latency_ms,
                model_reason: choice.reason,
            },
            Err(error) => DecisionResult {
                provider: format!("rules (fallback from {PROVIDER_NAME}: {error})"),
                latency_ms,
                ..rules
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::loops::LoopSignal;
    use harness_core::task::TaskState;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Serves one request on a loopback port with the given status and body. Returns the base URL, and a channel
    /// that receives the request head and body as text.
    fn serve_once(status: u16, body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, received) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                head.push_str(&line);
            }
            let mut payload = vec![0; length];
            reader.read_exact(&mut payload).unwrap();
            head.push_str(&String::from_utf8_lossy(&payload));
            sender.send(head).unwrap();
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });
        (format!("http://{address}"), received)
    }

    fn section(endpoint: &str, key_env: Option<&str>) -> JevSection {
        JevSection {
            enabled: true,
            endpoint: Some(endpoint.to_owned()),
            api_key_env: key_env.map(str::to_owned),
            timeout_ms: 5_000,
        }
    }

    fn env_with_key(name: &str) -> Option<String> {
        (name == "JEV_KEY").then(|| "secret-123".to_owned())
    }

    fn stalled() -> Vec<LoopSignal> {
        vec![LoopSignal::SameFailure {
            fingerprint: "fp_v1:aaaa".to_owned(),
            count: 3,
        }]
    }

    fn decide(engine: &JevEngine) -> DecisionResult {
        let state = TaskState::new("task-1", "repo", "fix calc");
        let signals = stalled();
        engine.decide(&DecisionInput {
            state: &state,
            signals: &signals,
        })
    }

    #[test]
    fn a_checked_answer_is_used_with_its_probability_and_the_key_goes_in_a_bearer_header() {
        let (url, received) = serve_once(
            200,
            r#"{"decision":"REPLAN","confidence":0.8,"reason":"make a new plan"}"#,
        );
        let engine =
            JevEngine::from_config(&section(&url, Some("JEV_KEY")), &env_with_key).unwrap();

        let result = decide(&engine);

        assert_eq!(result.decision, harness_core::decision::Decision::Replan);
        assert_eq!(result.confidence, Some(0.8));
        assert_eq!(result.provider, "jev");
        assert_eq!(result.model_reason.as_deref(), Some("make a new plan"));
        let head = received.recv().unwrap();
        assert!(head.starts_with("POST /v1/decisions "), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("authorization: bearer secret-123"),
            "{head}"
        );
        assert!(head.contains("bridle.loop_decision.v1"), "{head}");
        assert!(
            !head.contains("command"),
            "no command text leaves the machine: {head}"
        );
    }

    #[test]
    fn a_failed_call_falls_back_to_the_rules_and_never_shows_the_key() {
        let (url, _received) = serve_once(500, "boom");
        let engine =
            JevEngine::from_config(&section(&url, Some("JEV_KEY")), &env_with_key).unwrap();

        let result = decide(&engine);

        assert!(
            result
                .provider
                .starts_with("rules (fallback from jev: HTTP 500"),
            "{}",
            result.provider
        );
        assert!(!result.provider.contains("secret-123"));
        // The rules give 1.0 for a loop rule; the fallback keeps that rule confidence.
        assert_eq!(result.confidence, Some(1.0));
        assert!(!format!("{engine:?}").contains("secret-123"));
    }

    #[test]
    fn an_answer_outside_the_contract_falls_back_to_the_rules() {
        let (url, _received) = serve_once(200, r#"{"decision":"CONTINUE","confidence":0.9}"#);
        let engine = JevEngine::from_config(&section(&url, None), &env_with_key).unwrap();
        let result = decide(&engine);
        assert!(
            result.provider.starts_with("rules (fallback from jev:"),
            "{}",
            result.provider
        );
        assert!(result.provider.contains("CONTINUE"), "{}", result.provider);
    }

    #[test]
    fn an_unreachable_endpoint_falls_back_to_the_rules() {
        let engine =
            JevEngine::from_config(&section("http://127.0.0.1:9", None), &env_with_key).unwrap();
        let result = decide(&engine);
        assert!(
            result.provider.starts_with("rules (fallback from jev:"),
            "{}",
            result.provider
        );
    }

    #[test]
    fn a_key_the_config_names_must_be_set_before_any_call() {
        let missing = JevEngine::from_config(
            &section("https://jev.example.com", Some("ABSENT")),
            &env_with_key,
        );
        assert_eq!(
            missing.err().as_deref(),
            Some("environment variable ABSENT is not set")
        );
        let no_endpoint = JevEngine::from_config(
            &JevSection {
                endpoint: None,
                ..section("", None)
            },
            &env_with_key,
        );
        assert!(no_endpoint.is_err());
    }

    /// A live check of the TLS path, not of the decision service: a public host with a valid certificate. The path is
    /// not a decision endpoint, so the host answers with an error status. Getting that status proves the handshake.
    /// Run with `--ignored`, on a machine with network access.
    #[test]
    #[ignore = "needs network access"]
    fn tls_handshake_reaches_a_public_host() {
        let slow = JevSection {
            timeout_ms: 30_000,
            ..section("https://index.crates.io", None)
        };
        let engine = JevEngine::from_config(&slow, &env_with_key).unwrap();
        let result = decide(&engine);
        assert!(
            result
                .provider
                .starts_with("rules (fallback from jev: HTTP "),
            "{}",
            result.provider
        );
    }

    #[test]
    fn the_request_is_bounded_and_names_no_prompt_or_output() {
        let mut state = TaskState::new("task-1", "repo", "a long secret objective");
        state.last_action = Some("run_tests\u{7}tests/test_calc.py".to_owned());
        let signals = stalled();
        let body = JevEngine::request(&DecisionInput {
            state: &state,
            signals: &signals,
        });
        assert_eq!(body["schema"], SCHEMA);
        assert_eq!(body["state"]["phase"], "START");
        assert_eq!(body["state"]["last_action"], "run_teststests/test_calc.py");
        assert!(body.get("prompt").is_none());
        assert_eq!(body["choices"], json!(["ESCALATE", "ASK_USER", "REPLAN"]));
    }
}
