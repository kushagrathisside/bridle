//! The subagent tier, run: gathers the inputs a call names, asks the small model once, and checks the answer.
//!
//! Every path goes through the tool policy before it is read, and every input is redacted before it leaves this module,
//! so a secret in an allowed file still never reaches the small model. The parent gets the checked answer only.

use std::io::Read;
use std::path::Path;
use std::time::Instant;

use harness_core::config::{Config, Verdict};
use harness_core::security::redact::Redactor;
use harness_core::security::tool_policy::{ToolKind, ToolPolicy, ToolRequest};
use harness_core::subagent::{self, Answer, Input, Limits, Role};
use serde_json::json;

use crate::providers::{ChatMessage, ChatRequest, ModelProvider, provider_for};

/// Bytes read past the input limit, so a secret that straddles the cut is redacted whole before the cut.
const READ_MARGIN: usize = 4_096;
/// The longest task text a call accepts, in characters.
const MAX_TASK_CHARS: usize = 500;
/// The longest literal text a call accepts, in bytes.
const MAX_TEXT_BYTES: usize = 8_000;
/// The most tokens the small model may write for one answer.
const MAX_ANSWER_TOKENS: u32 = 600;

/// What a caller asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub role: Role,
    /// The question, or the instruction for a summary. Bounded by the caller.
    pub task: String,
    /// Files to read, relative to the workspace. Each goes through the tool policy.
    pub paths: Vec<String>,
    /// A literal text to condense, named `text` in the answer.
    pub text: Option<String>,
}

/// A checked answer and what the parent is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub answer: Answer,
    /// The bounded rendering the parent reads.
    pub rendered: String,
    /// The model slot's model id.
    pub model: String,
    pub inputs_left_out: usize,
    pub latency_ms: u64,
}

/// Reads the inputs a request names. A path the policy refuses is an error that names the rule, not a silent skip.
/// Each input is read with a margin, redacted, and returned unbounded; `answer` bounds it.
pub fn gather_inputs(
    req: &Request,
    workspace: &Path,
    policy: &ToolPolicy,
    redactor: &Redactor,
    limits: Limits,
) -> Result<Vec<Input>, String> {
    if req.paths.len() > limits.max_inputs {
        return Err(format!(
            "at most {} paths are read per call",
            limits.max_inputs
        ));
    }
    let mut inputs = Vec::new();
    for path in &req.paths {
        let mut tool = ToolRequest::new(ToolKind::Read);
        tool.paths = vec![path.clone()];
        let decision = policy.evaluate(&tool);
        if decision.verdict != Verdict::Allow {
            return Err(format!(
                "{path} was refused by policy ({}): {}",
                decision.rule,
                decision.reasons.join("; ")
            ));
        }
        let file = std::fs::File::open(workspace.join(path))
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        let mut bytes = Vec::new();
        file.take((limits.max_input_bytes + READ_MARGIN) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        inputs.push(Input {
            name: path.clone(),
            text: redactor.redact(&text).into_owned(),
        });
    }
    if let Some(text) = &req.text {
        inputs.push(Input {
            name: "text".to_owned(),
            text: redactor.redact(text).into_owned(),
        });
    }
    Ok(inputs)
}

/// Runs one subagent call from its configuration: the tier must be on, the slot must exist and be reachable, the task
/// and the text must be within bounds, and the inputs pass the policy. Shared by the `subagent` tool and `agent subagent`,
/// so the two cannot drift apart.
pub fn run_for_config(
    req: &Request,
    config: &Config,
    workspace: &Path,
    policy: &ToolPolicy,
    redactor: &Redactor,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Outcome, String> {
    if !config.subagents.enabled {
        return Err("the subagent tier is off; set [subagents] enabled = true".to_owned());
    }
    if req.task.trim().is_empty() || req.task.chars().count() > MAX_TASK_CHARS {
        return Err(format!("task must be 1 to {MAX_TASK_CHARS} characters"));
    }
    if req
        .text
        .as_ref()
        .is_some_and(|text| text.len() > MAX_TEXT_BYTES)
    {
        return Err(format!("text must be at most {MAX_TEXT_BYTES} bytes"));
    }
    let slot = &config.subagents.slot;
    let model = config
        .models
        .get(slot)
        .ok_or_else(|| format!("subagent slot `{slot}` is not defined under [models]"))?;
    let provider = provider_for(model, env)
        .map_err(|error| format!("subagent model `{slot}` is unavailable: {error}"))?;
    let limits = Limits {
        max_input_bytes: config.subagents.max_input_bytes,
        max_inputs: config.subagents.max_inputs,
    };
    let inputs = gather_inputs(req, workspace, policy, redactor, limits)?;
    answer(provider.as_ref(), req, inputs, limits)
}

/// The one model call of a subagent, and the check of its answer. An answer that breaks the contract is an error: the
/// parent is never shown a reply it cannot trust as an answer.
pub fn answer(
    provider: &dyn ModelProvider,
    req: &Request,
    inputs: Vec<Input>,
    limits: Limits,
) -> Result<Outcome, String> {
    let (inputs, inputs_left_out) = subagent::bound_inputs(inputs, limits);
    let names: Vec<&str> = inputs.iter().map(|input| input.name.as_str()).collect();
    let started = Instant::now();
    let response = provider
        .chat(&chat_request(req, &inputs))
        .map_err(|error| format!("the subagent model did not answer: {error}"))?;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let content = response
        .content
        .ok_or_else(|| "the subagent model gave no text".to_owned())?;
    let answer = subagent::parse_answer(&content, &names)
        .map_err(|reject| format!("the subagent answer broke the contract: {reject}"))?;
    let rendered = subagent::render(req.role, &answer, inputs_left_out);
    Ok(Outcome {
        answer,
        rendered,
        model: response.model,
        inputs_left_out,
        latency_ms,
    })
}

/// The chat request: the role's system prompt, then the task and each input, each fenced by its name.
fn chat_request(req: &Request, inputs: &[Input]) -> ChatRequest {
    let mut user = format!("Task: {}\n", req.task);
    for input in inputs {
        user.push_str(&format!(
            "\n--- input `{0}` ---\n{1}\n--- end of `{0}` ---\n",
            input.name, input.text
        ));
    }
    ChatRequest {
        messages: vec![
            ChatMessage::system(subagent::system_prompt(req.role)),
            ChatMessage::user(user),
        ],
        response_format: Some(json!({"type": "json_object"})),
        max_tokens: Some(MAX_ANSWER_TOKENS),
        enable_thinking: Some(false),
        ..ChatRequest::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::providers::{ChatResponse, MockProvider, ProviderError};
    use harness_core::config::SecuritySection;

    fn policy(dir: &Path) -> ToolPolicy {
        ToolPolicy::with_rules(dir, &SecuritySection::default(), false).unwrap()
    }

    fn limits() -> Limits {
        Limits {
            max_input_bytes: 4_096,
            max_inputs: 4,
        }
    }

    fn reply(text: &str) -> Result<ChatResponse, ProviderError> {
        Ok(ChatResponse {
            model: "lfm2.5-1.2b-thinking".to_owned(),
            content: Some(text.to_owned()),
            ..ChatResponse::default()
        })
    }

    fn explore(paths: &[&str]) -> Request {
        Request {
            role: Role::Explore,
            task: "what does add() return?".to_owned(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            text: None,
        }
    }

    #[test]
    fn a_cited_answer_reaches_the_parent_with_its_reference() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("calc.py"),
            "def add(a, b):\n    return a - b\n",
        )
        .unwrap();
        let req = explore(&["calc.py"]);
        let inputs = gather_inputs(
            &req,
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
            limits(),
        )
        .unwrap();
        let provider = MockProvider::scripted(
            "micro",
            vec![reply(
                r#"{"summary":"add subtracts","findings":[{"text":"returns a - b","ref":"calc.py:2"}]}"#,
            )],
        );

        let outcome = answer(&provider, &req, inputs, limits()).unwrap();

        assert_eq!(
            outcome.answer.findings[0].reference.as_deref(),
            Some("calc.py:2")
        );
        assert!(outcome.rendered.contains("- returns a - b (calc.py:2)"));
        assert_eq!(outcome.model, "lfm2.5-1.2b-thinking");
        let sent = provider.requests();
        assert_eq!(
            sent[0].response_format.as_ref().unwrap()["type"],
            "json_object"
        );
        assert!(
            sent[0].messages[1]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("--- input `calc.py` ---")
        );
    }

    #[test]
    fn a_policy_refused_path_is_an_error_and_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "API_KEY=supersecret\n").unwrap();
        let err = gather_inputs(
            &explore(&[".env"]),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
            limits(),
        )
        .unwrap_err();
        assert!(err.contains("refused by policy (secret_file)"), "{err}");
    }

    #[test]
    fn secrets_in_an_allowed_file_are_redacted_before_the_model_sees_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.txt"),
            "api_key = supersecret123\nname = bridle\n",
        )
        .unwrap();
        let inputs = gather_inputs(
            &explore(&["config.txt"]),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
            limits(),
        )
        .unwrap();
        assert!(
            !inputs[0].text.contains("supersecret123"),
            "{}",
            inputs[0].text
        );
        assert!(inputs[0].text.contains("name = bridle"));
    }

    #[test]
    fn a_long_input_is_bounded_and_the_parent_is_told() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), "x".repeat(20_000)).unwrap();
        let small = Limits {
            max_input_bytes: 512,
            max_inputs: 4,
        };
        let inputs = gather_inputs(
            &explore(&["big.txt"]),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
            small,
        )
        .unwrap();
        let provider = MockProvider::scripted("micro", vec![reply(r#"{"summary":"ok"}"#)]);
        let outcome = answer(&provider, &explore(&["big.txt"]), inputs, small).unwrap();
        assert!(
            provider.requests()[0].messages[1]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("[cut by the harness")
        );
        assert!(outcome.rendered.contains("[subagent explore answer"));
    }

    #[test]
    fn an_answer_outside_the_contract_is_an_error_not_an_answer() {
        let provider = MockProvider::scripted("micro", vec![reply("I think it is fine.")]);
        let err = answer(&provider, &explore(&[]), Vec::new(), limits()).unwrap_err();
        assert!(
            err.contains("broke the contract: the answer is not JSON"),
            "{err}"
        );
    }

    #[test]
    fn an_unreachable_model_is_an_error_that_names_it() {
        let provider = MockProvider::scripted(
            "micro",
            vec![Err(ProviderError::Unreachable(
                "connection refused".to_owned(),
            ))],
        );
        let err = answer(&provider, &explore(&[]), Vec::new(), limits()).unwrap_err();
        assert!(err.contains("the subagent model did not answer"), "{err}");
    }

    #[test]
    fn too_many_paths_are_refused_before_any_read() {
        let dir = tempfile::tempdir().unwrap();
        let tight = Limits {
            max_input_bytes: 4_096,
            max_inputs: 1,
        };
        let err = gather_inputs(
            &explore(&["a", "b"]),
            dir.path(),
            &policy(dir.path()),
            &Redactor::new(),
            tight,
        )
        .unwrap_err();
        assert!(err.contains("at most 1 paths"), "{err}");
    }
}
