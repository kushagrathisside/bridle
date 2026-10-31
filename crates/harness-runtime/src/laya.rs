//! The Laya decision backend: the requests for `POST /v1/systemone`, the check of the answer, and the HTTP call.
//!
//! Laya is a local, non-autoregressive decision model served by `laya-serve` (Apache 2.0; see ADR-0015). It answers a
//! typed question with one of the choices the question names, a probability for each, and a confidence. It does not
//! generate text, so an answer outside the named choices is a fault, not a suggestion.
//!
//! The server runs on the same machine. The client refuses any endpoint that is not loopback, and it does not use a
//! proxy. What it sends is bounded: the loop facts or the request text, redacted, and no repository content or tool
//! output.

use std::collections::BTreeMap;
use std::time::Duration;

use harness_core::security::redact::Redactor;
use serde_json::{Map, Value, json};

use crate::providers::openai_compat::{read_text, transport_error};

/// The path of the decision route on `laya-serve`.
pub const PATH: &str = "/v1/systemone";
/// The checkpoint that answers decisions. The server picks a checkpoint by the language of the text unless the request
/// names one; the decision checkpoint is named so a request is never answered by a checkpoint that is not on disk.
pub const DECISION_MODEL: &str = "typed-decisions";
/// The name of the question for a stalled loop.
pub const LOOP_QUESTION: &str = "loop";
/// The name of the question for routing a request.
pub const ROUTE_QUESTION: &str = "route";
/// The longest request text sent for routing, in characters.
pub const MAX_REQUEST_CHARS: usize = 2_000;

/// The answer to one choice question, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The request for a stalled loop: the state facts, the signal names, and the choices with what each one means.
pub fn loop_request(state: &Value, signals: &[&str], criteria: &[(&str, &str)]) -> Value {
    let state = json!({
        "phase": state["phase"],
        "iteration": state["iteration"],
        "failure_count": state["failure_count"],
        "consecutive_refusals": state["consecutive_refusals"],
        "last_action": state["last_action"],
        "signals": signals,
    });
    choice_request(
        LOOP_QUESTION,
        "The agent's loop has stalled. What should happen next?",
        state,
        criteria,
    )
}

/// The request for routing: the request text, redacted and bounded, and the two slots with what each one is for.
pub fn route_request(text: &str, criteria: &[(&str, &str)]) -> Value {
    let redacted = Redactor::new().redact(text);
    let bounded: String = redacted.chars().take(MAX_REQUEST_CHARS).collect();
    choice_request(
        ROUTE_QUESTION,
        "Which model should answer this request? Choose the slot whose work the request is.",
        json!({ "request": bounded }),
        criteria,
    )
}

fn choice_request(
    question: &str,
    instructions: &str,
    state: Value,
    criteria: &[(&str, &str)],
) -> Value {
    let criteria: Map<String, Value> = criteria
        .iter()
        .map(|(choice, meaning)| ((*choice).to_owned(), json!(meaning)))
        .collect();
    json!({
        "model": DECISION_MODEL,
        "state": state,
        "questions": {
            question: {
                "type": "choice",
                "instructions": instructions,
                "criteria": criteria,
            }
        }
    })
}

/// Checks an answer: the named question must be present and of type `choice`, its choice must be one of `choices`,
/// and the probabilities must name exactly those choices, each within 0 to 1.
pub fn parse(text: &str, question: &str, choices: &[&str]) -> Result<Answer, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("the answer is not JSON: {error}"))?;
    let answer = value
        .get("answers")
        .and_then(|answers| answers.get(question))
        .ok_or_else(|| format!("the answer has no `{question}` question"))?;
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        return Err("the answer is not of type choice".to_owned());
    }
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| "the answer has no choice".to_owned())?;
    if !choices.contains(&choice) {
        return Err(format!("the choice {choice:?} is not one of {choices:?}"));
    }
    let raw = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| "the answer has no probabilities".to_owned())?;
    let mut probabilities = BTreeMap::new();
    for (name, value) in raw {
        let p = value
            .as_f64()
            .filter(|p| (0.0..=1.0).contains(p))
            .ok_or_else(|| format!("the probability for {name:?} is not between 0 and 1"))?;
        probabilities.insert(name.clone(), p);
    }
    let named: Vec<&str> = probabilities.keys().map(String::as_str).collect();
    let mut expected: Vec<&str> = choices.to_vec();
    expected.sort_unstable();
    if named != expected {
        return Err(format!(
            "the probabilities name {named:?}, not {expected:?}"
        ));
    }
    let confidence = answer
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|c| (0.0..=1.0).contains(c))
        .ok_or_else(|| "the answer has no confidence between 0 and 1".to_owned())?;
    Ok(Answer {
        choice: choice.to_owned(),
        probabilities,
        confidence,
    })
}

/// Whether an endpoint is plain `http://` to a loopback host. The configuration checks the same rule (V42); the client
/// checks it again, so an endpoint is never used off the machine.
pub fn is_loopback_http(endpoint: &str) -> bool {
    endpoint.starts_with("http://") && harness_core::config::endpoint_is_loopback(endpoint)
}

/// A client for one Laya server. It holds no key: the server is on this machine and is not authenticated.
pub struct LayaClient {
    url: String,
    timeout: Duration,
    agent: ureq::Agent,
}

impl std::fmt::Debug for LayaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayaClient")
            .field("url", &self.url)
            .finish()
    }
}

impl LayaClient {
    /// Builds the client for an endpoint such as `http://127.0.0.1:8000`. Refuses anything that is not loopback HTTP.
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self, String> {
        if !is_loopback_http(endpoint) {
            return Err(format!(
                "the Laya endpoint {endpoint} is not a loopback http address"
            ));
        }
        Ok(Self {
            url: format!("{}{PATH}", endpoint.trim_end_matches('/')),
            timeout,
            agent: crate::http::local_agent(timeout),
        })
    }

    /// Posts one request and checks the answer under `question` against `choices`.
    pub fn ask(&self, body: &Value, question: &str, choices: &[&str]) -> Result<Answer, String> {
        let mut response = self
            .agent
            .post(self.url.as_str())
            .send_json(body)
            .map_err(|error| transport_error(error, self.timeout).to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        let text =
            read_text(response.body_mut(), self.timeout).map_err(|error| error.to_string())?;
        parse(&text, question, choices)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// The answer `laya-serve` returned for a same-failure state, with the typed-decisions checkpoint (2026-10-31).
    const RECORDED: &str = r#"{"model":"laya-rl-agent","answers":{"loop":{"type":"choice","choice":"ESCALATE",
        "probabilities":{"ESCALATE":0.564,"ASK_USER":0.2359,"REPLAN":0.2001},"confidence":0.7,
        "answer_confidence":0.564,"action":{"act_probability":1.0}}},"routing":{"model":"typed-decisions"}}"#;

    const CHOICES: [&str; 3] = ["ESCALATE", "ASK_USER", "REPLAN"];

    #[test]
    fn the_recorded_answer_is_read() {
        let answer = parse(RECORDED, "loop", &CHOICES).unwrap();
        assert_eq!(answer.choice, "ESCALATE");
        assert_eq!(answer.probabilities.len(), 3);
        assert!((answer.probabilities["ESCALATE"] - 0.564).abs() < 1e-9);
        assert!((answer.confidence - 0.7).abs() < 1e-9);
    }

    #[test]
    fn a_choice_outside_the_named_choices_is_rejected() {
        let text = RECORDED.replace("\"choice\":\"ESCALATE\"", "\"choice\":\"PLAN\"");
        assert!(parse(&text, "loop", &CHOICES).unwrap_err().contains("PLAN"));
    }

    #[test]
    fn probabilities_must_name_exactly_the_choices() {
        let text = RECORDED.replace("\"REPLAN\":0.2001", "\"PLAN\":0.2001");
        assert!(
            parse(&text, "loop", &CHOICES)
                .unwrap_err()
                .contains("probabilities name")
        );
    }

    #[test]
    fn a_probability_outside_zero_to_one_is_rejected() {
        let text = RECORDED.replace("\"ESCALATE\":0.564", "\"ESCALATE\":1.5");
        assert!(
            parse(&text, "loop", &CHOICES)
                .unwrap_err()
                .contains("between 0 and 1")
        );
    }

    #[test]
    fn a_missing_question_or_a_non_choice_answer_is_rejected() {
        assert!(
            parse("{\"answers\":{}}", "loop", &CHOICES)
                .unwrap_err()
                .contains("no `loop` question")
        );
        let text = RECORDED.replace("\"type\":\"choice\"", "\"type\":\"score\"");
        assert!(
            parse(&text, "loop", &CHOICES)
                .unwrap_err()
                .contains("not of type choice")
        );
    }

    #[test]
    fn the_loop_request_names_the_question_the_choices_and_the_state_only() {
        let state = json!({"phase": "DIAGNOSE", "iteration": 2, "failure_count": 3,
            "consecutive_refusals": 0, "last_action": "run_tests", "prompt": "secret text"});
        let body = loop_request(
            &state,
            &["same_failure"],
            &[("ESCALATE", "a stronger model should look")],
        );
        let question = &body["questions"][LOOP_QUESTION];
        assert_eq!(question["type"], "choice");
        assert_eq!(
            question["criteria"]["ESCALATE"],
            "a stronger model should look"
        );
        assert_eq!(body["state"]["signals"][0], "same_failure");
        assert!(body["state"].get("prompt").is_none(), "{body}");
    }

    #[test]
    fn every_request_names_the_decision_checkpoint() {
        let body = route_request("fix the typo", &[("MAIN", "everyday work")]);
        assert_eq!(body["model"], DECISION_MODEL);
        let body = loop_request(&json!({"phase": "DIAGNOSE"}), &["same_failure"], &[]);
        assert_eq!(body["model"], DECISION_MODEL);
    }

    #[test]
    fn the_route_request_is_redacted_and_bounded() {
        let body = route_request(
            "Design the cache. The key is api_key = supersecret123.",
            &[("MAIN", "everyday work"), ("DEEP", "a wider view")],
        );
        let text = body["state"]["request"].as_str().unwrap();
        assert!(text.starts_with("Design the cache."), "{text}");
        assert!(!text.contains("supersecret123"), "{text}");
        assert!(body["questions"][ROUTE_QUESTION]["criteria"]["DEEP"].is_string());
        let long = route_request(&"x".repeat(5_000), &[]);
        assert_eq!(
            long["state"]["request"].as_str().unwrap().chars().count(),
            MAX_REQUEST_CHARS
        );
    }

    #[test]
    fn only_loopback_http_endpoints_are_accepted() {
        assert!(is_loopback_http("http://127.0.0.1:8000"));
        assert!(is_loopback_http("http://localhost:8000/"));
        assert!(!is_loopback_http("http://10.0.0.5:8000"));
        assert!(!is_loopback_http("https://127.0.0.1:8000"));
        assert!(!is_loopback_http("http://127.0.0.1.evil.example:8000"));
        assert!(LayaClient::new("http://example.com:8000", Duration::from_secs(1)).is_err());
    }

    /// Reads one whole request: the headers, then the body that `Content-Length` announces. A single read can return
    /// only the headers, and answering before the body arrives can reset the connection under the client.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&request).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    /// A one-shot server that answers the first request with `body`, and returns the request it read.
    fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (addr, handle)
    }

    #[test]
    fn the_client_posts_to_the_route_and_reads_the_checked_answer() {
        let (endpoint, server) = serve_once(RECORDED);
        let client = LayaClient::new(&endpoint, Duration::from_secs(5)).unwrap();
        let body = loop_request(&json!({"phase": "DIAGNOSE"}), &["same_failure"], &[]);
        let answer = client.ask(&body, LOOP_QUESTION, &CHOICES).unwrap();
        assert_eq!(answer.choice, "ESCALATE");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/systemone"), "{request}");
    }
}
