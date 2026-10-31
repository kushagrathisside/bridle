//! The chat agent behind the control center's home screen.
//!
//! A turn sends the conversation and the tool list to a model slot, runs the tool calls the model asks
//! for, feeds the results back, and repeats until the model answers without a tool call.
//!
//! The chat is another host, not a privileged one. Every tool call goes through the same functions the
//! OpenCode hook uses: [`hook::answer_with`] gives the policy's verdict, the orchestrator's loop rules and
//! the event-log entry before the call, and the redaction and the task record after it. The tools run
//! through the MCP server's own call path, so verification results are recorded and every output is
//! redacted exactly as it is for OpenCode. A blocked call is answered to the model with the rule that
//! blocked it, and `ask` counts as blocked. There is no way to approve a blocked call from here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_core::config::ModelConfig;
use harness_core::identity;
use harness_core::security::redact::Redactor;
use harness_runtime::mcp::{McpTool, Server, ServerInfo, ToolContext, file_tools};
use harness_runtime::providers::{
    ChatMessage, ChatRequest, ModelProvider, ProviderError, Role, StreamEvent, ToolCall, ToolSpec,
    provider_for,
};
use serde_json::{Value, json};

use crate::app::App;
use crate::commands::hook;
use crate::commands::mcp::tools_for;
use crate::host::opencode_v1::HookEvent;
use crate::tui::events::sanitize;

/// Model calls in one turn at most. A turn that needs more stops and says so.
pub const MAX_STEPS: usize = 30;
/// Characters of one tool result sent back to the model.
const MAX_TOOL_CHARS: usize = 12_000;
/// Characters of a tool call's arguments or result shown to the user.
const SUMMARY_CHARS: usize = 110;
const PREVIEW_CHARS: usize = 240;
/// How often progress of a tool call or of reasoning is reported at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Characters per token assumed when sizing the context window. Rough, and on the safe side.
const CHARS_PER_TOKEN: usize = 3;

const SYSTEM_PROMPT: &str = "\
You are a coding assistant working in a repository through Bridle, a local-first harness. \
Use the tools to look at and change the repository; do not guess what a file contains.\n\
- Read a file with read_file before you edit it. Change files with edit_file, using small exact edits; \
use write_file only for a new file or a complete rewrite.\n\
- Find things with search_text, list_files, repo_map and symbol_search instead of reading everything.\n\
- After you change code, run the project's checks with run_tests or run_verification when they are \
configured, and fix what fails.\n\
- Every tool call is checked by a security policy. If a call is blocked, do not look for a way around \
it: say what you wanted to do and why, and ask the user how to continue.\n\
- Never read or write secret files such as .env or key files.\n\
- Keep answers short: say what you did and what you found.";

/// What happened during a turn, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A piece of the answer, as it streams.
    Delta(String),
    /// A tool call that passed the policy and is about to run.
    ToolStart {
        id: String,
        name: String,
        summary: String,
    },
    /// A tool call that did not run, or that the tool itself refused under the policy.
    ToolBlocked {
        id: String,
        name: String,
        summary: String,
        rule: String,
        message: String,
    },
    /// A tool call that ran. `ok` is false when the tool reported an error.
    ToolDone {
        id: String,
        name: String,
        ok: bool,
        preview: String,
    },
    /// Something the user should know that is not part of the answer.
    Notice(String),
    /// What the model is doing while it is not writing text, such as writing a long tool call. It is
    /// shown on the status line and not kept.
    Working(String),
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// The model answered without asking for another tool.
    Done,
    /// The user stopped the turn.
    Stopped,
    /// [`MAX_STEPS`] model calls were used.
    StepLimit,
    /// The model could not be reached or answered badly.
    Failed(String),
}

/// One conversation: its history, its tools and the session the orchestrator records it under.
pub struct Agent {
    app: App,
    server: Server,
    specs: Vec<ToolSpec>,
    messages: Vec<ChatMessage>,
    session_id: String,
    next_rpc: u64,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("session_id", &self.session_id)
            .field("messages", &self.messages.len())
            .finish()
    }
}

impl Agent {
    /// The application this agent runs in: the configuration, the workspace and the policy.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Builds the agent for a loaded application. Refuses a configuration with errors, as the hook and
    /// `agent mcp` do.
    pub fn new(app: App) -> Result<Self, String> {
        app.require_valid_config().map_err(|e| format!("{e:#}"))?;
        let policy = app.policy().map_err(|e| format!("{e:#}"))?;
        let ctx = ToolContext {
            workspace: app.workspace.clone(),
            config: app.config.clone(),
            verification: app.config.verification.clone(),
            policy,
            redactor: Redactor::new(),
        };
        let mut tools = tools_for(&app);
        tools.extend(file_tools());
        let specs = tools.iter().map(|tool| spec_of(tool.as_ref())).collect();
        let info = ServerInfo {
            name: identity::MCP_SERVER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            instructions: String::new(),
        };
        Ok(Self {
            server: Server::new(info, tools, ctx),
            specs,
            messages: Vec::new(),
            session_id: new_session_id(),
            next_rpc: 1,
            app,
        })
    }

    /// The provider and the configuration of a model slot.
    pub fn provider(&self, slot: &str) -> Result<(Box<dyn ModelProvider>, ModelConfig), String> {
        let model = self
            .app
            .config
            .models
            .get(slot)
            .ok_or_else(|| format!("no model slot named `{slot}`"))?
            .clone();
        let provider = provider_for(&model, &|key| std::env::var(key).ok())
            .map_err(|error| describe_error(&error, &model))?;
        Ok((provider, model))
    }

    /// The names of the tools the model is offered.
    pub fn tool_names(&self) -> Vec<&str> {
        self.specs.iter().map(|s| s.name.as_str()).collect()
    }

    /// Runs one turn: `user` goes in, events come out through `emit`. `cancel` is checked between
    /// model calls and tool calls, so a stop takes effect after the step in progress.
    pub fn run_turn(
        &mut self,
        provider: &dyn ModelProvider,
        model: &ModelConfig,
        user: &str,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event),
    ) -> TurnEnd {
        self.messages.push(ChatMessage::user(user));
        for _ in 0..MAX_STEPS {
            if cancel.load(Ordering::Relaxed) {
                return TurnEnd::Stopped;
            }
            let (request, dropped) = self.request(model);
            if dropped > 0 {
                emit(Event::Notice(format!(
                    "{dropped} older message(s) were dropped to fit the context window"
                )));
            }
            let mut throttle = Throttle::default();
            let response = provider.chat_stream_with(&request, &mut |event| {
                // A stop drops the connection, so the server stops generating at once.
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                match event {
                    StreamEvent::Text(piece) => emit(Event::Delta(piece.to_owned())),
                    StreamEvent::ToolCall { name, chars } => {
                        if throttle.ready(name) {
                            let what = if name.is_empty() {
                                "a tool".to_owned()
                            } else {
                                sanitize(name)
                            };
                            emit(Event::Working(format!(
                                "writing a call to {what} ({chars} characters so far)"
                            )));
                        }
                    }
                    StreamEvent::Reasoning { chars } => {
                        if throttle.ready("reasoning") {
                            emit(Event::Working(format!(
                                "thinking ({chars} characters of reasoning)"
                            )));
                        }
                    }
                }
                true
            });
            let response = match response {
                Ok(response) => response,
                Err(error) => return TurnEnd::Failed(describe_error(&error, model)),
            };
            if response.finish_reason.as_deref() == Some("stopped") {
                // The stream was abandoned at the user's request: keep what was said, run nothing.
                let said = response
                    .content
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "(stopped before answering)".to_owned());
                self.messages.push(ChatMessage {
                    role: Role::Assistant,
                    content: Some(said),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
                return TurnEnd::Stopped;
            }
            let cut_off = response.finish_reason.as_deref() == Some("length");
            if cut_off {
                emit(Event::Notice(
                    "the answer was cut off by the token limit (max_output_tokens)".to_owned(),
                ));
            }
            if response.tool_calls.is_empty() {
                if response.content.as_deref().is_none_or(str::is_empty) {
                    emit(Event::Notice(
                        "the model returned an empty answer".to_owned(),
                    ));
                }
                self.messages.push(ChatMessage {
                    role: Role::Assistant,
                    content: response.content,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
                return TurnEnd::Done;
            }
            let calls = response.tool_calls.clone();
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: response.content,
                tool_calls: response.tool_calls,
                tool_call_id: None,
            });
            for (index, call) in calls.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    // Every call the model made needs an answer, or the history is not valid.
                    for rest in &calls[index..] {
                        self.push_result(rest, "not run: the user stopped the turn");
                    }
                    return TurnEnd::Stopped;
                }
                let truncated = cut_off && index + 1 == calls.len();
                let text = self.run_call(call, index, truncated, model, emit);
                self.push_result(call, &text);
            }
        }
        TurnEnd::StepLimit
    }

    fn push_result(&mut self, call: &ToolCall, text: &str) {
        self.messages.push(ChatMessage {
            role: Role::Tool,
            content: Some(text.to_owned()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call.id.clone()),
        });
    }

    /// The request for the next model call, with the history made to fit, and how many messages that
    /// dropped.
    fn request(&mut self, model: &ModelConfig) -> (ChatRequest, usize) {
        let mut system = format!(
            "{SYSTEM_PROMPT}\nWorkspace: {}",
            self.app.workspace.display()
        );
        // The same answer the OpenCode hook gives for the system prompt: the repository memory and the task state.
        // A part that cannot be read is logged by the hook path and left out, so the chat and the hook agree.
        let answer = hook::answer_with(
            &self.app,
            HookEvent::SystemTransform,
            &json!({ "sessionID": self.session_id }),
        );
        if let Some(block) = answer["text"].as_str() {
            system.push_str("\n\n");
            system.push_str(block);
        }
        let schema_chars: usize = self
            .specs
            .iter()
            .map(|s| s.name.len() + s.description.len() + s.parameters.to_string().len())
            .sum();
        let window = usize::try_from(model.context).unwrap_or(usize::MAX) * CHARS_PER_TOKEN;
        let budget = (window / 10 * 7)
            .saturating_sub(schema_chars + system.len())
            .max(2_000);
        let dropped = compact(&mut self.messages, budget);
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        messages.push(ChatMessage::system(system));
        messages.extend(self.messages.iter().cloned());
        let request = ChatRequest {
            messages,
            tools: self.specs.clone(),
            tool_choice: Some(json!("auto")),
            ..ChatRequest::default()
        };
        (request, dropped)
    }

    /// Runs one tool call and returns the text the model gets back.
    fn run_call(
        &mut self,
        call: &ToolCall,
        index: usize,
        truncated: bool,
        model: &ModelConfig,
        emit: &mut dyn FnMut(Event),
    ) -> String {
        let name = call.name.clone();
        // What the screen shows: the model chose this text, so it never carries control characters.
        let shown = sanitize(&name);
        let id = if call.id.is_empty() {
            format!("call_{index}")
        } else {
            sanitize(&call.id)
        };
        let args: Value = if call.arguments.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    let text = if truncated {
                        format!(
                            "your reply reached the output limit ({} tokens) in the middle of this call, so its arguments are incomplete and nothing was run. Make the call smaller: for a large file, write it in several parts",
                            model.max_output_tokens
                        )
                    } else {
                        format!(
                            "the arguments were not valid JSON ({error}); call the tool again with a JSON object"
                        )
                    };
                    emit(Event::ToolDone {
                        id,
                        name: shown,
                        ok: false,
                        preview: text.clone(),
                    });
                    return text;
                }
            }
        };
        if !args.is_object() {
            let text = "the arguments must be a JSON object".to_owned();
            emit(Event::ToolDone {
                id,
                name: shown,
                ok: false,
                preview: text.clone(),
            });
            return text;
        }
        if !self.specs.iter().any(|s| s.name == name) {
            let text = format!(
                "unknown tool `{}`; the tools are: {}",
                sanitize(&name),
                self.tool_names().join(", ")
            );
            emit(Event::ToolDone {
                id,
                name: shown,
                ok: false,
                preview: sanitize(&text),
            });
            return text;
        }

        let hook_id = hook_call_id(&id, index);
        let (hook_tool, hook_args) = hook_view(&name, &args);
        let before = json!({
            "sessionID": self.session_id,
            "callID": hook_id,
            "tool": hook_tool,
            "args": hook_args,
        });
        let summary = self.summarise(&name, &args);

        let verdict = hook::answer_with(&self.app, HookEvent::ToolBefore, &before);
        if verdict["action"] != "allow" {
            let message = verdict["message"]
                .as_str()
                .unwrap_or("blocked by policy")
                .to_owned();
            emit(Event::ToolBlocked {
                id,
                name,
                summary,
                rule: verdict["rule"].as_str().unwrap_or("policy").to_owned(),
                message: sanitize(&message),
            });
            return message;
        }

        emit(Event::ToolStart {
            id: id.clone(),
            name: name.clone(),
            summary: summary.clone(),
        });
        let started = Instant::now();
        let output = self.call_tool(&name, &args);
        let mut text = output.text;
        let after = json!({
            "sessionID": self.session_id,
            "callID": hook_id,
            "tool": before["tool"],
            "args": before["args"],
            "output": text,
            "title": name,
            "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        let answer = hook::answer_with(&self.app, HookEvent::ToolAfter, &after);
        if let Some(redacted) = answer.get("output").and_then(Value::as_str) {
            text = redacted.to_owned();
        }
        let text = cut(&text, MAX_TOOL_CHARS);
        if output.blocked {
            emit(Event::ToolBlocked {
                id,
                name,
                summary,
                rule: output.rule.unwrap_or_else(|| "policy".to_owned()),
                message: sanitize(&text),
            });
        } else {
            emit(Event::ToolDone {
                id,
                name,
                ok: !output.is_error,
                preview: preview(&text),
            });
        }
        text
    }

    /// Calls a tool through the MCP server's own `tools/call` path.
    fn call_tool(&mut self, name: &str, args: &Value) -> ToolReply {
        let rpc = self.next_rpc;
        self.next_rpc += 1;
        let line = json!({
            "jsonrpc": "2.0",
            "id": rpc,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
        .to_string();
        let Some(reply) = self.server.handle_line(&line) else {
            return ToolReply::error("the tool server gave no answer");
        };
        let Ok(reply) = serde_json::from_str::<Value>(&reply) else {
            return ToolReply::error("the tool server gave an unreadable answer");
        };
        if let Some(error) = reply.get("error") {
            let message = error["message"].as_str().unwrap_or("tool error");
            return ToolReply::error(message);
        }
        let result = &reply["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let structured = result.get("structuredContent");
        let blocked = structured
            .and_then(|s| s.get("status"))
            .and_then(Value::as_str)
            == Some("blocked");
        ToolReply {
            text,
            is_error: result["isError"].as_bool().unwrap_or(false),
            blocked,
            rule: structured
                .and_then(|s| s.get("rule"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    /// A short, redacted description of a call for the screen.
    fn summarise(&self, name: &str, args: &Value) -> String {
        let text = |key: &str| args.get(key).and_then(Value::as_str);
        let detail = match name {
            "read_file" | "write_file" | "edit_file" | "list_files" | "file_summary" => {
                text("path").unwrap_or(".").to_owned()
            }
            "search_text" => format!(
                "\"{}\" in {}",
                text("pattern").unwrap_or(""),
                text("path").unwrap_or(".")
            ),
            "run_command" => {
                let words: Vec<&str> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                format!("{} {}", text("program").unwrap_or("?"), words.join(" "))
            }
            _ => {
                let json = args.to_string();
                if json == "{}" { String::new() } else { json }
            }
        };
        let redacted = self.server.ctx.redactor.redact(&detail).into_owned();
        cut(&sanitize(&redacted), SUMMARY_CHARS)
    }
}

/// What one tool call came back with.
struct ToolReply {
    text: String,
    is_error: bool,
    /// The tool itself refused the call under the policy (for example `run_command`).
    blocked: bool,
    rule: Option<String>,
}

impl ToolReply {
    fn error(message: &str) -> Self {
        Self {
            text: message.to_owned(),
            is_error: true,
            blocked: false,
            rule: None,
        }
    }
}

/// Lets a progress report through when its subject changed or enough time has passed.
#[derive(Debug, Default)]
struct Throttle {
    last: Option<(Instant, String)>,
}

impl Throttle {
    fn ready(&mut self, subject: &str) -> bool {
        let due = self
            .last
            .as_ref()
            .is_none_or(|(at, last)| last != subject || at.elapsed() >= PROGRESS_EVERY);
        if due {
            self.last = Some((Instant::now(), subject.to_owned()));
        }
        due
    }
}

fn spec_of(tool: &dyn McpTool) -> ToolSpec {
    ToolSpec {
        name: tool.name().to_owned(),
        description: tool.description().to_owned(),
        parameters: tool.input_schema(),
    }
}

/// The call as the OpenCode hook would see it, so the policy classifies it the same way: the file tools
/// map to OpenCode's own file tools, every other tool to an MCP tool of this server.
fn hook_view(name: &str, args: &Value) -> (String, Value) {
    let get = |key: &str| args.get(key).cloned().unwrap_or(Value::Null);
    match name {
        "read_file" => (
            "read".to_owned(),
            json!({ "filePath": get("path"), "offset": get("offset"), "limit": get("limit") }),
        ),
        "list_files" => (
            "list".to_owned(),
            json!({ "path": args.get("path").cloned().unwrap_or_else(|| json!(".")) }),
        ),
        // `codesearch` is a search the policy checks by path; `grep` would also read the pattern as a path.
        "search_text" => (
            "codesearch".to_owned(),
            json!({
                "pattern": get("pattern"),
                "path": args.get("path").cloned().unwrap_or_else(|| json!(".")),
            }),
        ),
        "write_file" => (
            "write".to_owned(),
            json!({ "filePath": get("path"), "content": get("content") }),
        ),
        "edit_file" => (
            "edit".to_owned(),
            json!({
                "filePath": get("path"),
                "oldString": get("old_string"),
                "newString": get("new_string"),
                "replaceAll": get("replace_all"),
            }),
        ),
        other => (
            format!("{}_{other}", identity::MCP_SERVER_NAME),
            args.clone(),
        ),
    }
}

/// A call id the state files can hold: letters, digits, `-` and `_`, at most 64 characters.
fn hook_call_id(id: &str, index: usize) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if cleaned.is_empty() {
        format!("call_{index}")
    } else {
        cleaned
    }
}

fn new_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("ses_chat_{millis}")
}

/// `text` cut to `max` characters, with a note of how much was left out.
fn cut(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(&format!(
        "\n... [{} more characters not shown]",
        total - max
    ));
    out
}

/// The first lines of a result, on one line, for the screen.
fn preview(text: &str) -> String {
    let first: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect();
    let joined = first.join(" | ");
    let clean = sanitize(&joined);
    if clean.chars().count() > PREVIEW_CHARS {
        let mut cut: String = clean.chars().take(PREVIEW_CHARS).collect();
        cut.push('…');
        cut
    } else {
        clean
    }
}

fn describe_error(error: &ProviderError, model: &ModelConfig) -> String {
    match error {
        ProviderError::Unreachable(detail) => format!(
            "cannot reach the server at {} ({detail}). Open the Models view (Tab, then 6) and press s to start it, or check the endpoint in the configuration",
            model.endpoint
        ),
        ProviderError::Timeout(after) => format!(
            "the server did not answer within {after:?}. A large model may still be loading; try again in a moment"
        ),
        ProviderError::Http { status, body } => {
            format!("the server answered HTTP {status}: {}", sanitize(body))
        }
        ProviderError::InvalidResponse(detail) => {
            format!(
                "the server's answer could not be read: {}",
                sanitize(detail)
            )
        }
        ProviderError::MissingApiKey(var) => {
            format!("the environment variable {var} is not set")
        }
        ProviderError::Other(detail) => sanitize(detail),
    }
}

fn message_chars(message: &ChatMessage) -> usize {
    message.content.as_deref().map_or(0, str::len)
        + message
            .tool_calls
            .iter()
            .map(|c| c.name.len() + c.arguments.len())
            .sum::<usize>()
}

fn total_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_chars).sum()
}

/// Makes the history fit `budget` characters: first the older tool results are shortened, then the oldest
/// turns are dropped, then the newer results are shortened too. A turn is dropped whole, from its user
/// message to the next one, so a tool result never loses its call. Returns how many messages were dropped.
fn compact(messages: &mut Vec<ChatMessage>, budget: usize) -> usize {
    let shorten = |messages: &mut [ChatMessage], keep_last: usize, to: usize| {
        let end = messages.len().saturating_sub(keep_last);
        for message in &mut messages[..end] {
            if message.role == Role::Tool
                && let Some(text) = &message.content
                && text.chars().count() > to
            {
                let head: String = text.chars().take(to).collect();
                message.content = Some(format!("{head}… [older output shortened]"));
            }
        }
    };
    if total_chars(messages) <= budget {
        return 0;
    }
    shorten(messages, 6, 300);
    let mut dropped = 0;
    while total_chars(messages) > budget {
        let next_user = messages
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, m)| m.role == Role::User)
            .map(|(i, _)| i);
        let Some(next_user) = next_user else { break };
        messages.drain(..next_user);
        dropped += next_user;
    }
    if total_chars(messages) > budget {
        shorten(messages, 2, 120);
    }
    dropped
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_runtime::providers::{ChatResponse, HealthStatus, MockProvider};
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    const CONFIG: &str = r#"
profile = "test"
[models.main]
provider = "mock"
id = "mock-model"
role = "engineer"
endpoint = "http://127.0.0.1:9/v1"
context = 8192
max_output_tokens = 512
hardware = "cpu"
[context]
target_tokens = 4000
hard_limit = 8000
"#;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(extra_config: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let ws = dir.path().join("ws");
            fs::create_dir_all(ws.join("src")).unwrap();
            fs::write(
                ws.join("src/main.rs"),
                "fn main() {\n    println!(\"hi\");\n}\n",
            )
            .unwrap();
            fs::write(ws.join("notes.txt"), "hello from the notes\n").unwrap();
            fs::write(ws.join(".env"), "API_KEY=sk-live-0123456789abcdefghij\n").unwrap();
            fs::write(
                dir.path().join("cfg.toml"),
                format!("{CONFIG}{extra_config}"),
            )
            .unwrap();
            Self { dir }
        }

        fn ws(&self) -> std::path::PathBuf {
            dunce::canonicalize(self.dir.path().join("ws")).unwrap()
        }

        fn agent(&self) -> Agent {
            let app = App::load(Some(&self.dir.path().join("cfg.toml")), Some(&self.ws())).unwrap();
            Agent::new(app).unwrap()
        }

        fn model(&self) -> ModelConfig {
            let app = App::load(Some(&self.dir.path().join("cfg.toml")), Some(&self.ws())).unwrap();
            app.config.models["main"].clone()
        }

        fn log(&self) -> String {
            let dir = self.ws().join(".local-agent");
            ["tool-events.jsonl", "events.jsonl"]
                .iter()
                .filter_map(|f| fs::read_to_string(dir.join(f)).ok())
                .collect()
        }
    }

    fn tool_call(id: &str, name: &str, args: Value) -> ChatResponse {
        ChatResponse {
            model: "mock-model".to_owned(),
            tool_calls: vec![ToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments: args.to_string(),
            }],
            ..ChatResponse::default()
        }
    }

    fn answer(text: &str) -> ChatResponse {
        ChatResponse {
            model: "mock-model".to_owned(),
            content: Some(text.to_owned()),
            ..ChatResponse::default()
        }
    }

    fn turn(
        fx: &Fixture,
        agent: &mut Agent,
        script: Vec<ChatResponse>,
        user: &str,
    ) -> (TurnEnd, Vec<Event>, MockProvider) {
        let provider = MockProvider::scripted("mock-model", script.into_iter().map(Ok).collect());
        let mut events = Vec::new();
        let cancel = AtomicBool::new(false);
        let end = agent.run_turn(&provider, &fx.model(), user, &cancel, &mut |e| {
            events.push(e)
        });
        (end, events, provider)
    }

    fn roles(agent: &Agent) -> Vec<Role> {
        agent.messages.iter().map(|m| m.role).collect()
    }

    #[test]
    fn a_tool_call_runs_and_the_answer_follows() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let (end, events, provider) = turn(
            &fx,
            &mut agent,
            vec![
                tool_call("c1", "read_file", json!({"path": "notes.txt"})),
                answer("The notes say hello."),
            ],
            "what is in notes.txt?",
        );
        assert_eq!(end, TurnEnd::Done);
        assert!(
            matches!(&events[0], Event::ToolStart { name, summary, .. } if name == "read_file" && summary == "notes.txt")
        );
        assert!(
            matches!(&events[1], Event::ToolDone { ok: true, preview, .. } if preview.contains("hello from the notes"))
        );
        assert!(events.iter().any(|e| matches!(e, Event::Delta(_))));
        assert_eq!(
            roles(&agent),
            [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
        );
        // The second request carries the tool result, and the first one offered the tools.
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].tools.iter().any(|t| t.name == "read_file"));
        assert!(requests[0].tools.iter().any(|t| t.name == "git_status"));
        let tool_msg = requests[1]
            .messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        assert!(
            tool_msg
                .content
                .as_deref()
                .unwrap()
                .contains("hello from the notes")
        );
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("c1"));
        // The same call went through the hook's logging.
        let log = fx.log();
        assert!(
            log.contains("\"tool_before\"") && log.contains("\"allowed\""),
            "{log}"
        );
        assert!(log.contains("\"tool_after\""), "{log}");
    }

    #[test]
    fn a_secret_file_is_blocked_and_the_model_is_told_the_rule() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let (end, events, provider) = turn(
            &fx,
            &mut agent,
            vec![
                tool_call("c1", "read_file", json!({"path": ".env"})),
                answer("I could not read it."),
            ],
            "show me .env",
        );
        assert_eq!(end, TurnEnd::Done);
        assert!(
            matches!(&events[0], Event::ToolBlocked { rule, message, .. } if rule == "secret_file" && message.contains("never sent to a model")),
            "{events:?}"
        );
        let requests = provider.requests();
        let told = requests[1]
            .messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .unwrap();
        assert!(
            told.content
                .as_deref()
                .unwrap()
                .starts_with("blocked by policy")
        );
        let log = fx.log();
        assert!(
            log.contains("\"deny\"") && log.contains("secret_file"),
            "{log}"
        );
        assert!(
            !log.contains("sk-live-0123456789abcdefghij"),
            "a secret reached the log"
        );
    }

    #[test]
    fn write_and_edit_change_files_through_the_policy() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let (end, events, _) = turn(
            &fx,
            &mut agent,
            vec![
                tool_call(
                    "c1",
                    "write_file",
                    json!({"path": "out/new.txt", "content": "one\n"}),
                ),
                tool_call(
                    "c2",
                    "edit_file",
                    json!({"path": "src/main.rs", "old_string": "\"hi\"", "new_string": "\"hello\""}),
                ),
                tool_call(
                    "c3",
                    "write_file",
                    json!({"path": "local-agent.toml", "content": "x"}),
                ),
                answer("done"),
            ],
            "make changes",
        );
        assert_eq!(end, TurnEnd::Done);
        assert_eq!(
            fs::read_to_string(fx.ws().join("out/new.txt")).unwrap(),
            "one\n"
        );
        assert!(
            fs::read_to_string(fx.ws().join("src/main.rs"))
                .unwrap()
                .contains("\"hello\"")
        );
        assert!(
            !fx.ws().join("local-agent.toml").exists(),
            "a control file must not be written"
        );
        let blocked: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::ToolBlocked { .. }))
            .collect();
        assert_eq!(blocked.len(), 1, "{events:?}");
    }

    #[test]
    fn bad_calls_are_answered_not_fatal() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let bad_json = ChatResponse {
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: "{not json".into(),
            }],
            ..ChatResponse::default()
        };
        let (end, events, provider) = turn(
            &fx,
            &mut agent,
            vec![
                bad_json,
                tool_call("c2", "no_such_tool", json!({})),
                answer("ok"),
            ],
            "go",
        );
        assert_eq!(end, TurnEnd::Done);
        assert!(
            matches!(&events[0], Event::ToolDone { ok: false, preview, .. } if preview.contains("not valid JSON"))
        );
        assert!(
            matches!(&events[1], Event::ToolDone { ok: false, preview, .. } if preview.contains("unknown tool"))
        );
        let last = provider.requests().pop().unwrap();
        let results: Vec<_> = last
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(results.len(), 2, "every call is answered");
    }

    #[test]
    fn a_hostile_tool_name_is_shown_without_control_characters() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let (_, events, _) = turn(
            &fx,
            &mut agent,
            vec![
                tool_call("c\u{1b}1", "evil\u{1b}[2Jtool", json!({})),
                answer("ok"),
            ],
            "go",
        );
        for event in &events {
            if let Event::ToolDone { id, name, .. } = event {
                assert!(
                    !name.contains('\u{1b}') && !id.contains('\u{1b}'),
                    "{event:?}"
                );
            }
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ToolDone { ok: false, .. }))
        );
    }

    /// A provider that streams by hand through `chat_stream_with` and gives up as soon as the caller says so.
    struct Streaming {
        events: Vec<(String, usize, bool)>,
        finish: ChatResponse,
        reported: AtomicUsize,
    }

    impl ModelProvider for Streaming {
        fn model_id(&self) -> &str {
            "mock-model"
        }
        fn health(&self) -> Result<HealthStatus, ProviderError> {
            Ok(HealthStatus {
                ok: true,
                detail: "ok".to_owned(),
            })
        }
        fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }
        fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ProviderError> {
            Ok(self.finish.clone())
        }
        fn chat_stream(
            &self,
            _: &ChatRequest,
            _: &mut dyn FnMut(&str),
        ) -> Result<ChatResponse, ProviderError> {
            Ok(self.finish.clone())
        }
        fn chat_stream_with(
            &self,
            _: &ChatRequest,
            on_event: &mut dyn FnMut(StreamEvent<'_>) -> bool,
        ) -> Result<ChatResponse, ProviderError> {
            let mut said = String::new();
            for (text, chars, is_call) in &self.events {
                self.reported.fetch_add(1, Ordering::SeqCst);
                let keep = if *is_call {
                    on_event(StreamEvent::ToolCall {
                        name: text,
                        chars: *chars,
                    })
                } else {
                    said.push_str(text);
                    on_event(StreamEvent::Text(text))
                };
                if !keep {
                    return Ok(ChatResponse {
                        content: Some(said),
                        finish_reason: Some("stopped".to_owned()),
                        ..ChatResponse::default()
                    });
                }
            }
            Ok(self.finish.clone())
        }
    }

    #[test]
    fn a_stop_in_the_middle_of_a_stream_ends_the_turn_at_once_with_a_valid_history() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let provider = Streaming {
            events: (0..50).map(|n| (format!("w{n} "), 0, false)).collect(),
            finish: answer("never reached"),
            reported: AtomicUsize::new(0),
        };
        let cancel = AtomicBool::new(false);
        let mut shown = 0;
        let end = agent.run_turn(
            &provider,
            &fx.model(),
            "talk a lot",
            &cancel,
            &mut |event| {
                if matches!(event, Event::Delta(_)) {
                    shown += 1;
                    if shown == 3 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            },
        );
        assert_eq!(end, TurnEnd::Stopped);
        assert!(
            provider.reported.load(Ordering::SeqCst) < 10,
            "the stream was abandoned, not read to the end"
        );
        assert_eq!(shown, 3, "nothing is shown after the stop");
        let last = agent.messages.last().unwrap();
        assert_eq!(last.role, Role::Assistant);
        assert!(
            last.content.as_deref().unwrap().starts_with("w0 w1 w2"),
            "{last:?}"
        );
        assert!(last.tool_calls.is_empty());
    }

    #[test]
    fn a_tool_call_being_written_is_reported_as_progress_and_throttled() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let provider = Streaming {
            events: vec![
                (String::new(), 0, true),
                ("write_file".to_owned(), 10, true),
                ("write_file".to_owned(), 99, true),
                ("write_file".to_owned(), 500, true),
            ],
            finish: answer("done"),
            reported: AtomicUsize::new(0),
        };
        let cancel = AtomicBool::new(false);
        let mut working = Vec::new();
        let end = agent.run_turn(
            &provider,
            &fx.model(),
            "write a big file",
            &cancel,
            &mut |event| {
                if let Event::Working(text) = event {
                    working.push(text);
                }
            },
        );
        assert_eq!(end, TurnEnd::Done);
        assert_eq!(
            working.len(),
            2,
            "a new subject is reported at once, the rest is throttled: {working:?}"
        );
        assert!(
            working[0].contains("a tool") && working[1].contains("write_file (10 characters"),
            "{working:?}"
        );
    }

    #[test]
    fn a_tool_call_cut_off_by_the_output_limit_is_explained_to_the_model() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let cut = ChatResponse {
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "write_file".into(),
                arguments: "{\"path\":\"a.txt\",\"content\":\"xxxx".into(),
            }],
            finish_reason: Some("length".into()),
            ..ChatResponse::default()
        };
        let (end, events, provider) = turn(
            &fx,
            &mut agent,
            vec![cut, answer("I will do it in parts")],
            "write a huge file",
        );
        assert_eq!(end, TurnEnd::Done);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Notice(t) if t.contains("token limit")))
        );
        let told = provider.requests()[1]
            .messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .and_then(|m| m.content.clone())
            .unwrap();
        assert!(
            told.contains("output limit (512 tokens)") && told.contains("several parts"),
            "{told}"
        );
        assert!(
            !fx.ws().join("a.txt").exists(),
            "nothing was written from an incomplete call"
        );
    }

    #[test]
    fn a_command_the_tool_refuses_is_shown_as_blocked() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let (_, events, _) = turn(
            &fx,
            &mut agent,
            vec![
                tool_call(
                    "c1",
                    "run_command",
                    json!({"program": "curl", "args": ["https://example.com"]}),
                ),
                answer("blocked"),
            ],
            "fetch it",
        );
        assert!(
            events.iter().any(|e| matches!(e, Event::ToolBlocked { message, .. } if message.contains("blocked by policy"))),
            "{events:?}"
        );
    }

    #[test]
    fn stopping_keeps_the_history_valid() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let two_calls = ChatResponse {
            tool_calls: vec![
                ToolCall {
                    id: "a".into(),
                    name: "list_files".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "b".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"notes.txt\"}".into(),
                },
            ],
            ..ChatResponse::default()
        };
        let provider = MockProvider::scripted("mock-model", vec![Ok(two_calls)]);
        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        let end = agent.run_turn(&provider, &fx.model(), "look", &cancel, &mut |e| {
            if matches!(e, Event::ToolDone { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            events.push(e);
        });
        assert_eq!(end, TurnEnd::Stopped);
        let calls = agent
            .messages
            .iter()
            .filter(|m| !m.tool_calls.is_empty())
            .map(|m| m.tool_calls.len())
            .sum::<usize>();
        let results = agent
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .count();
        assert_eq!((calls, results), (2, 2), "every call has an answer");
        assert!(
            agent
                .messages
                .last()
                .unwrap()
                .content
                .as_deref()
                .unwrap()
                .contains("stopped")
        );
    }

    #[test]
    fn a_turn_that_never_ends_stops_at_the_step_limit() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let script = (0..MAX_STEPS + 2)
            .map(|n| {
                tool_call(
                    &format!("c{n}"),
                    "list_files",
                    json!({"depth": 1 + (n % 3)}),
                )
            })
            .collect();
        let (end, _, provider) = turn(&fx, &mut agent, script, "loop");
        assert_eq!(end, TurnEnd::StepLimit);
        assert_eq!(provider.requests().len(), MAX_STEPS);
    }

    #[test]
    fn provider_errors_become_readable_failures() {
        let fx = Fixture::new("");
        let mut agent = fx.agent();
        let provider = MockProvider::scripted(
            "mock-model",
            vec![Err(ProviderError::Unreachable("connection refused".into()))],
        );
        let cancel = AtomicBool::new(false);
        let end = agent.run_turn(&provider, &fx.model(), "hi", &cancel, &mut |_| {});
        match end {
            TurnEnd::Failed(text) => {
                assert!(
                    text.contains("cannot reach the server") && text.contains("Models view"),
                    "{text}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn with_the_workflow_on_the_session_becomes_a_task_the_next_request_names() {
        let fx = Fixture::new("[workflow]\nenabled = true\n");
        let mut agent = fx.agent();
        let (end, _, provider) = turn(
            &fx,
            &mut agent,
            vec![tool_call("c1", "list_files", json!({})), answer("listed")],
            "list the repo",
        );
        assert_eq!(end, TurnEnd::Done);
        let second = &provider.requests()[1];
        let system = second.messages[0].content.as_deref().unwrap();
        assert!(system.contains("[harness task state"), "{system}");
        let state = harness_runtime::state::StateDir::open_existing(&fx.ws()).unwrap();
        assert_eq!(
            state.task_ids().unwrap().len(),
            1,
            "the chat session is a real task"
        );
    }

    #[test]
    fn the_loop_rule_refuses_endless_edits_in_the_chat_as_it_does_for_opencode() {
        let fx = Fixture::new("[workflow]\nenabled = true\n");
        let mut agent = fx.agent();
        let edit = |n: usize, from: &str, to: &str| {
            tool_call(
                &format!("e{n}"),
                "edit_file",
                json!({"path": "notes.txt", "old_string": from, "new_string": to}),
            )
        };
        let mut script = Vec::new();
        for n in 0..4 {
            script.push(edit(n * 2, "hello", "HELLO"));
            script.push(edit(n * 2 + 1, "HELLO", "hello"));
        }
        script.push(answer("stopped editing"));
        let (end, events, provider) = turn(&fx, &mut agent, script, "keep editing");
        assert_eq!(end, TurnEnd::Done);
        let refusal = events
            .iter()
            .find_map(|e| match e {
                Event::ToolBlocked { rule, message, .. } if rule == "loop_rule" => Some(message),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no loop refusal in {events:#?}"));
        assert!(!refusal.is_empty());
        // The model was told, and the refusal is in the event log like any other block.
        let told = provider.requests().iter().any(|r| {
            r.messages.iter().any(|m| {
                m.role == Role::Tool
                    && m.content
                        .as_deref()
                        .is_some_and(|c| c.contains(refusal.as_str()))
            })
        });
        assert!(told, "the model must see the refusal");
        assert!(fx.log().contains("loop_rule"), "{}", fx.log());
    }

    #[test]
    fn long_histories_are_compacted_without_orphaning_results() {
        let big = "x".repeat(4000);
        let mut messages = Vec::new();
        for turn in 0..4 {
            messages.push(ChatMessage::user(format!("question {turn}")));
            messages.push(ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: format!("t{turn}"),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }],
                tool_call_id: None,
            });
            messages.push(ChatMessage {
                role: Role::Tool,
                content: Some(big.clone()),
                tool_calls: vec![],
                tool_call_id: Some(format!("t{turn}")),
            });
            messages.push(ChatMessage {
                role: Role::Assistant,
                content: Some("ok".into()),
                tool_calls: vec![],
                tool_call_id: None,
            });
        }
        let dropped = compact(&mut messages, 6_000);
        assert!(dropped > 0);
        assert_eq!(
            messages[0].role,
            Role::User,
            "the history still starts at a question"
        );
        let calls: Vec<_> = messages
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        let answered: Vec<_> = messages
            .iter()
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        assert_eq!(calls, answered, "calls and results stay paired");
        assert_eq!(compact(&mut messages.clone(), usize::MAX), 0);
    }

    #[test]
    fn hook_view_maps_file_tools_to_the_hosts_own_tool_names() {
        assert_eq!(hook_view("read_file", &json!({"path": "a"})).0, "read");
        assert_eq!(
            hook_view("edit_file", &json!({"path": "a"})).1["filePath"],
            "a"
        );
        assert_eq!(
            hook_view("search_text", &json!({"pattern": "../x"})).0,
            "codesearch"
        );
        assert_eq!(
            hook_view("git_status", &json!({})).0,
            "local-agent_git_status"
        );
        assert_eq!(hook_call_id("call/1 ✓", 3), "call_1__");
        assert_eq!(hook_call_id("", 3), "call_3");
        let long = "a".repeat(200);
        assert_eq!(hook_call_id(&long, 0).len(), 64);
    }

    #[test]
    fn the_tool_list_is_the_servers_plus_the_file_tools() {
        let fx = Fixture::new("");
        let agent = fx.agent();
        let names = agent.tool_names();
        for expected in [
            "git_status",
            "run_tests",
            "run_command",
            "repo_map",
            "read_file",
            "edit_file",
            "write_file",
            "list_files",
            "search_text",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        assert!(Path::new(&fx.ws()).exists());
    }
}
