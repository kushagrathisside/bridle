//! OpenCode V1 host adapter (pinned 1.19.3). The ONLY Rust module that knows OpenCode's tool
//! names, argument shapes, permission payloads and config format (ADR-0001, R-33.2).
//!
//! Three jobs:
//! 1. translate OpenCode tool calls and permission requests into host-neutral
//!    [`ToolRequest`]s for `harness-core`'s [`ToolPolicy`];
//! 2. answer the plugin shim's hook calls (`agent hook <event>`) with JSON verdicts;
//! 3. generate `opencode.json` from the harness config, so there is one source of truth (C-11).

use std::path::Path;

use clap::ValueEnum;
use harness_core::config::{Config, ModelConfig, Verdict};
use harness_core::context::{self, ResultView};
use harness_core::identity;
use harness_core::security::redact::{Redactor, is_sensitive_entry};
use harness_core::security::tool_policy::{PolicyDecision, ToolKind, ToolPolicy, ToolRequest};
use harness_core::telemetry::{Event, EventKind};
use harness_runtime::mcp::record::is_verification_tool;
use serde_json::{Map, Value, json};

use crate::workflow::is_test_command;

/// Hook events the shim forwards. Names are the CLI spelling (`agent hook tool-before`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookEvent {
    ToolBefore,
    ToolAfter,
    Permission,
    ShellEnv,
    Event,
    /// `experimental.chat.messages.transform`: the per-request view of tool results (M6).
    MessagesTransform,
    /// `experimental.chat.system.transform`: the task state for the system prompt (M6).
    SystemTransform,
}

// ---------------------------------------------------------------------------------------------
// Tool-call translation
// ---------------------------------------------------------------------------------------------

/// Maps an OpenCode tool call (`input.tool`, `output.args`) to a host-neutral request.
pub fn map_tool_call(tool: &str, args: &Value) -> ToolRequest {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let mcp_prefix = format!("{}_", identity::MCP_SERVER_NAME);
    let mut req = match tool {
        "read" => ToolRequest::new(ToolKind::Read),
        "list" => ToolRequest::new(ToolKind::List),
        "glob" | "grep" | "codesearch" => ToolRequest::new(ToolKind::Search),
        "edit" | "multiedit" => ToolRequest::new(ToolKind::Edit),
        "write" => ToolRequest::new(ToolKind::Write),
        "patch" | "apply_patch" => ToolRequest::new(ToolKind::Patch),
        "bash" => ToolRequest::new(ToolKind::Shell),
        "webfetch" => ToolRequest::new(ToolKind::WebFetch),
        "websearch" => ToolRequest::new(ToolKind::WebSearch),
        "task" => ToolRequest::new(ToolKind::Subagent),
        "todowrite" | "todoread" | "question" | "skill" => ToolRequest::new(ToolKind::Todo),
        t if t.starts_with(&mcp_prefix) => {
            ToolRequest::new(ToolKind::Harness(t[mcp_prefix.len()..].to_owned()))
        }
        other => ToolRequest::new(ToolKind::Other(other.to_owned())),
    };
    // File paths, under every argument name OpenCode tools use.
    for key in ["filePath", "file_path", "path", "filepath"] {
        if let Some(p) = s(key).filter(|p| !p.is_empty()) {
            req.paths.push(p);
        }
    }
    if matches!(tool, "glob" | "grep")
        && let Some(base) = s("pattern").as_deref().and_then(glob_base)
    {
        req.paths.push(base);
    }
    // A glob pattern and a grep include filter are file-name globs, so the whole text is judged for secret names
    // (`.env*`). A grep pattern is a regular expression, not a path, and is not judged as one (M9).
    let glob_key = match tool {
        "glob" => Some("pattern"),
        "grep" => Some("include"),
        _ => None,
    };
    if let Some(key) = glob_key
        && let Some(text) = s(key).filter(|p| !p.is_empty())
    {
        req.paths.push(text);
    }
    if let Some(patch) = s("patchText").or_else(|| s("patch")) {
        req.paths.extend(patch_paths(&patch));
    }
    if tool == "bash" {
        req.command = s("command");
        req.workdir = s("workdir").or_else(|| s("cwd"));
    }
    req
}

/// The literal directory prefix of a glob pattern, when it could point outside the workspace
/// (absolute, or containing `..`). `None` for ordinary relative patterns.
fn glob_base(pattern: &str) -> Option<String> {
    let cut = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let literal = &pattern[..cut];
    let literal = literal.rsplit_once(['/', '\\']).map_or("", |(dir, _)| dir);
    let risky = harness_core::config::is_absolute_any_platform(Path::new(literal))
        || literal.split(['/', '\\']).any(|c| c == "..");
    (risky && !literal.is_empty()).then(|| literal.to_owned())
}

/// Files named in an OpenCode `apply_patch` envelope.
fn patch_paths(patch: &str) -> Vec<String> {
    const MARKERS: [&str; 4] = [
        "*** Add File: ",
        "*** Update File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ];
    patch
        .lines()
        .filter_map(|l| MARKERS.iter().find_map(|m| l.trim_start().strip_prefix(m)))
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Maps an OpenCode permission request (`Permission { type, pattern, metadata, title }`).
pub fn map_permission(permission: &Value) -> ToolRequest {
    let kind = permission.get("type").and_then(Value::as_str).unwrap_or("");
    let meta = permission.get("metadata").cloned().unwrap_or(Value::Null);
    let patterns: Vec<String> = match permission.get("pattern") {
        Some(Value::String(p)) => vec![p.clone()],
        Some(Value::Array(ps)) => ps
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    let meta_str = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned);
    match kind {
        "bash" => {
            let mut req = ToolRequest::new(ToolKind::Shell);
            req.command = meta_str("command")
                .or_else(|| patterns.first().cloned())
                .or_else(|| {
                    permission
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            req
        }
        "external_directory" => {
            let mut req = ToolRequest::new(ToolKind::Read);
            req.paths = meta_str("filepath")
                .or_else(|| meta_str("filePath"))
                .or_else(|| meta_str("path"))
                .map(|p| vec![p])
                .unwrap_or(patterns);
            req
        }
        other => {
            let mut req = map_tool_call(other, &meta);
            if req.paths.is_empty() && !matches!(req.kind, ToolKind::Shell) {
                req.paths = patterns.into_iter().filter(|p| !p.contains('*')).collect();
            }
            req
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bash allow-list shared by the generated config and the bypass check
// ---------------------------------------------------------------------------------------------

/// Command prefixes OpenCode may run without asking (they are READ_ONLY or EXECUTE_SAFE in
/// `command_risk`). Every other command falls through to `"*": "ask"`.
pub const BASH_ALLOW: &[&str] = &[
    "git status*",
    "git diff*",
    "git log*",
    "git show*",
    "git rev-parse*",
    "git ls-files*",
    "git blame*",
    "ls*",
    "dir*",
    "pwd",
    "cat *",
    "head *",
    "tail *",
    "wc *",
    "grep *",
    "rg *",
    "which *",
    "where *",
    "echo *",
    "pytest*",
    "python -m pytest*",
    "python -m unittest*",
    "cargo test*",
    "cargo build*",
    "cargo check*",
    "cargo clippy*",
    "cargo fmt*",
    "ruff check*",
    "ruff format --check*",
    "mypy *",
    "pyright*",
    "ctest*",
    "cmake --build*",
    "go test*",
    "go build*",
    "go vet*",
    "tsc*",
    "nvidia-smi*",
];

/// Commands the explore agent may run (strictly read-only).
pub const BASH_EXPLORE_ALLOW: &[&str] = &[
    "git status*",
    "git diff*",
    "git log*",
    "git show*",
    "git ls-files*",
    "git blame*",
    "ls*",
    "dir*",
    "pwd",
    "cat *",
    "head *",
    "tail *",
    "wc *",
    "grep *",
    "rg *",
];

/// OpenCode wildcard semantics: `*` matches any run of characters, `?` exactly one.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Allow patterns in effect for the build agent: the built-in list plus configured safe commands.
pub fn build_allow_patterns(cfg: &Config) -> Vec<String> {
    BASH_ALLOW
        .iter()
        .map(|s| (*s).to_owned())
        .chain(cfg.security.safe_commands.iter().map(|c| format!("{c}*")))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Hook handlers
// ---------------------------------------------------------------------------------------------

/// Everything a hook handler needs. Built once per hook process.
#[derive(Debug)]
pub struct HookContext<'a> {
    pub config: &'a Config,
    pub policy: &'a ToolPolicy,
    pub redactor: &'a Redactor,
}

/// Dispatches one hook call. Returns the JSON answer for the shim plus the telemetry event.
pub fn handle(event: HookEvent, input: &Value, ctx: &HookContext<'_>) -> (Value, Option<Event>) {
    match event {
        HookEvent::ToolBefore => tool_before(input, ctx),
        HookEvent::ToolAfter => tool_after(input, ctx),
        HookEvent::Permission => permission(input, ctx),
        HookEvent::ShellEnv => (shell_env(std::env::vars()), None),
        HookEvent::Event => (json!({}), Some(session_event(input))),
        HookEvent::MessagesTransform => messages_transform(input, ctx),
        HookEvent::SystemTransform => (json!({}), None),
    }
}

fn base_event(kind: EventKind, input: &Value) -> Event {
    let mut e = Event::now(kind);
    e.session_id = input
        .get("sessionID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    e.call_id = input
        .get("callID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    e.tool = input.get("tool").and_then(Value::as_str).map(str::to_owned);
    e
}

fn decision_event(mut e: Event, d: &PolicyDecision) -> Event {
    e.tool_class = Some(d.class);
    e.verdict = Some(d.verdict);
    e.with("rule", d.rule.clone())
        .with("reasons", d.reasons.clone())
}

fn tool_before(input: &Value, ctx: &HookContext<'_>) -> (Value, Option<Event>) {
    let tool = input.get("tool").and_then(Value::as_str).unwrap_or("");
    let args = input.get("args").cloned().unwrap_or(Value::Null);
    let req = map_tool_call(tool, &args);
    let mut decision = ctx.policy.evaluate(&req);

    // OpenCode only asks the user when no allow pattern matched. If one did (e.g. `pytest*`
    // matching `pytest && curl x | sh`) but the full command needs approval, the user was never
    // asked, so block and tell the model to run the risky part on its own.
    if decision.verdict == Verdict::Ask
        && let Some(cmd) = req.command.as_deref()
        && build_allow_patterns(ctx.config)
            .iter()
            .any(|p| wildcard_match(p, cmd.trim()))
    {
        decision.verdict = Verdict::Deny;
        decision.rule = "allow_pattern_bypass".to_owned();
        decision.reasons.push(
            "this command matches an auto-approved prefix but contains a step that needs \
             approval; run that step as a separate command so it can be approved"
                .to_owned(),
        );
    }

    // A reason can echo part of the command, such as `unrecognised program <word>`, so it is
    // redacted before it reaches the model's message or the event log.
    for reason in &mut decision.reasons {
        let redacted = ctx.redactor.redact(reason).into_owned();
        *reason = redacted;
    }

    // Decision D3: `Ask` is never allowed. Under `--auto` OpenCode approves anything we do not
    // deny, so only an explicit Allow gets through.
    let allow = decision.verdict == Verdict::Allow;
    let message = if allow {
        String::new()
    } else {
        format!(
            "blocked by policy ({}, {}): {}",
            decision.class,
            decision.rule,
            decision.reasons.join("; ")
        )
    };
    let mut ev = decision_event(base_event(EventKind::ToolBefore, input), &decision);
    ev.status = Some(if allow { "allowed" } else { "blocked" }.to_owned());
    if let Some(cmd) = &req.command {
        ev = ev.with("command", ctx.redactor.redact(cmd).into_owned());
    }
    if !req.paths.is_empty() {
        ev = ev.with("paths", req.paths.clone());
    }
    let answer = json!({
        "action": if allow { "allow" } else { "block" },
        "message": message,
        "class": decision.class,
        "verdict": decision.verdict,
        "rule": decision.rule,
    });
    (answer, Some(ev))
}

fn tool_after(input: &Value, ctx: &HookContext<'_>) -> (Value, Option<Event>) {
    let raw = input.get("output").and_then(Value::as_str).unwrap_or("");
    let redacted = if ctx.config.security.secrets_redaction {
        ctx.redactor.redact(raw)
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    let changed = redacted.as_ref() != raw;
    let mut ev = base_event(EventKind::ToolAfter, input);
    ev.duration_ms = input.get("durationMs").and_then(Value::as_u64);
    ev.status = Some("completed".to_owned());
    ev = ev
        .with("output_bytes", raw.len())
        .with("redacted", changed)
        .with(
            "output_preview",
            redacted.chars().take(400).collect::<String>(),
        );
    if let Some(title) = input.get("title").and_then(Value::as_str) {
        ev = ev.with("title", title);
    }
    let answer = if changed {
        json!({ "output": redacted })
    } else {
        json!({})
    };
    (answer, Some(ev))
}

fn permission(input: &Value, ctx: &HookContext<'_>) -> (Value, Option<Event>) {
    let perm = input.get("permission").cloned().unwrap_or(Value::Null);
    let req = map_permission(&perm);
    let decision = ctx.policy.evaluate(&req);
    // Decision D3: `Ask` maps to deny, so `--auto` cannot approve a call we did not vet.
    let status = match decision.verdict {
        Verdict::Allow => "allow",
        Verdict::Ask | Verdict::Deny => "deny",
    };
    let mut ev = decision_event(base_event(EventKind::Permission, &perm), &decision);
    ev.status = Some(status.to_owned());
    ev = ev.with(
        "permission_type",
        perm.get("type").cloned().unwrap_or(Value::Null),
    );
    (
        json!({ "status": status, "reason": decision.reasons.join("; ") }),
        Some(ev),
    )
}

/// Blanks every sensitive environment variable for shell tools (R-27.2). OpenCode merges the
/// returned values over the inherited environment, so an empty string removes the secret.
pub fn shell_env(vars: impl Iterator<Item = (String, String)>) -> Value {
    let set: Map<String, Value> = vars
        .filter(|(k, v)| !v.is_empty() && is_sensitive_entry(k, v))
        .map(|(k, _)| (k, Value::String(String::new())))
        .collect();
    json!({ "set": set })
}

fn session_event(input: &Value) -> Event {
    let ev = input.get("event").cloned().unwrap_or(Value::Null);
    let kind = ev
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let props = ev.get("properties").cloned().unwrap_or(Value::Null);
    let mut e = Event::now(EventKind::Session);
    e.session_id = props
        .get("sessionID")
        .or_else(|| props.get("info").and_then(|i| i.get("id")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    e.status = Some(kind);
    if let Some(file) = props.get("file").and_then(Value::as_str) {
        e = e.with("file", file);
    }
    if let Some(err) = props.get("error") {
        e = e.with("error", err.clone());
    }
    e
}

/// Answer used when the hook cannot even build its context (bad config, bad workspace).
/// Policy hooks fail closed; telemetry hooks are no-ops.
pub fn failure_answer(event: HookEvent, error: &str) -> Value {
    match event {
        HookEvent::ToolBefore => json!({
            "action": "block",
            "message": format!("{} policy engine unavailable: {error}", identity::PRODUCT_ID),
        }),
        // Decision D3: an unanswered permission is a deny, never a question `--auto` approves.
        HookEvent::Permission => json!({ "status": "deny", "reason": error }),
        HookEvent::ShellEnv => shell_env(std::env::vars()),
        HookEvent::ToolAfter
        | HookEvent::Event
        | HookEvent::MessagesTransform
        | HookEvent::SystemTransform => json!({}),
    }
}

// ---------------------------------------------------------------------------------------------
// opencode.json generation
// ---------------------------------------------------------------------------------------------

/// The slowest harness MCP call: `git_diff` runs up to four git commands of 30 s each.
const MCP_GIT_WORST_CASE_MS: u64 = 4 * 30_000;
/// Slack on top of the slowest call, for process start-up and output handling.
const MCP_TIMEOUT_MARGIN_MS: u64 = 30_000;

/// OpenCode 1.19.3 applies the MCP `timeout` to every request, tool calls included. A
/// `run_verification` call runs every configured step in turn, so the timeout covers the sum of
/// the step timeouts, and never less than the slowest git tool.
fn mcp_timeout_ms(cfg: &Config) -> u64 {
    let steps: u64 = cfg
        .verification
        .steps
        .iter()
        .map(|step| {
            step.timeout_ms
                .unwrap_or(cfg.verification.default_timeout_ms)
        })
        .sum();
    steps.max(MCP_GIT_WORST_CASE_MS) + MCP_TIMEOUT_MARGIN_MS
}

/// OpenCode provider id for a model slot (`local-agent-main`).
pub fn provider_id(slot: &str) -> String {
    format!("{}-{slot}", identity::PROVIDER_PREFIX)
}

/// `provider/model` reference OpenCode uses in `model` fields.
pub fn model_ref(slot: &str, model: &ModelConfig) -> String {
    format!("{}/{}", provider_id(slot), model.id)
}

/// The key OpenCode sends to a slot's endpoint. A slot with `api_key_env` reads the key from that variable when OpenCode
/// starts, so the key is never written into the generated file. Other slots get a placeholder: llama-server ignores
/// the key, and the AI SDK needs a non-empty value.
fn api_key_reference(m: &ModelConfig) -> String {
    match &m.api_key_env {
        Some(var) => format!("{{env:{var}}}"),
        None => "unused".to_owned(),
    }
}

fn provider_entry(slot: &str, m: &ModelConfig) -> Value {
    let name = m.display_name.clone().unwrap_or_else(|| m.id.clone());
    json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": format!("{} {slot} ({name})", identity::PRODUCT_ID),
        "options": {
            "baseURL": m.endpoint,
            "apiKey": api_key_reference(m),
            "timeout": m.timeout_ms,
        },
        "models": {
            m.id.clone(): {
                "name": name,
                "tool_call": m.capabilities.tool_calling,
                "reasoning": m.reasoning.enabled,
                "temperature": true,
                "attachment": m.capabilities.vision,
                "limit": { "context": m.context, "output": m.max_output_tokens },
            }
        }
    })
}

fn agent_entry(model: Option<String>, m: Option<&ModelConfig>) -> Map<String, Value> {
    let mut a = Map::new();
    if let Some(r) = model {
        a.insert("model".into(), Value::String(r));
    }
    if let Some(t) = m.and_then(|m| m.sampling.temperature) {
        a.insert("temperature".into(), json!(t));
    }
    if let Some(p) = m.and_then(|m| m.sampling.top_p) {
        a.insert("top_p".into(), json!(p));
    }
    a
}

fn bash_rules(default: &str, allow: impl Iterator<Item = String>) -> Value {
    // OpenCode evaluates pattern rules in order with later matches overriding earlier ones, so
    // the catch-all comes first.
    let mut rules = Map::new();
    rules.insert("*".into(), Value::String(default.to_owned()));
    for p in allow {
        rules.insert(p, Value::String("allow".into()));
    }
    Value::Object(rules)
}

/// Read rules: deny obvious secret files before OpenCode even asks the hook (defence in depth;
/// the hook also blocks them).
fn read_rules() -> Value {
    json!({
        "*": "allow",
        "*.env": "deny",
        "*.env.*": "deny",
        "*.env.example": "allow",
        "*.pem": "deny",
        "*.key": "deny",
        "*id_rsa*": "deny",
        "*id_ed25519*": "deny",
    })
}

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::Ask => "ask",
        Verdict::Deny => "deny",
    }
}

/// Builds `opencode.json` for `cfg`. `agent_bin` is the absolute path of this executable, used to
/// launch the MCP server.
pub fn generate_opencode_config(cfg: &Config, agent_bin: &Path) -> Value {
    let mut providers = Map::new();
    for (slot, m) in &cfg.models {
        providers.insert(provider_id(slot), provider_entry(slot, m));
    }
    let main = cfg.models.get(&cfg.agent.default_model);
    let main_ref = main.map(|m| model_ref(&cfg.agent.default_model, m));
    let review_slot = cfg
        .models
        .iter()
        .find(|(_, m)| m.role == harness_core::config::ModelRole::Review);
    let deep_slot = cfg
        .models
        .iter()
        .find(|(_, m)| m.role == harness_core::config::ModelRole::DeepReasoning);

    let mut agents = Map::new();

    let mut build = agent_entry(main_ref.clone(), main);
    build.insert(
        "permission".into(),
        json!({
            "edit": "allow",
            "read": read_rules(),
            "bash": bash_rules("ask", build_allow_patterns(cfg).into_iter()),
            "webfetch": verdict_str(cfg.security.network),
            "websearch": verdict_str(cfg.security.network),
            "external_directory": "deny",
        }),
    );
    build.insert("steps".into(), json!(cfg.agent.max_iterations.max(1) * 10));
    agents.insert("build".into(), Value::Object(build));

    // explore: read-only evidence gathering (C-2: runs on the main model until M4 measures LFM).
    let mut explore = agent_entry(main_ref.clone(), main);
    explore.insert("mode".into(), json!("subagent"));
    explore.insert(
        "description".into(),
        json!(
            "Read-only repository exploration. Returns a compact evidence summary: findings, \
               file/symbol references, unresolved questions."
        ),
    );
    explore.insert("prompt".into(), json!(EXPLORE_PROMPT));
    explore.insert(
        "permission".into(),
        json!({
            "edit": "deny",
            "read": read_rules(),
            "bash": bash_rules("deny", BASH_EXPLORE_ALLOW.iter().map(|s| (*s).to_owned())),
            "webfetch": "deny",
            "websearch": "deny",
            "external_directory": "deny",
        }),
    );
    agents.insert("explore".into(), Value::Object(explore));

    if let Some((slot, m)) = review_slot {
        let mut review = agent_entry(Some(model_ref(slot, m)), Some(m));
        review.insert("mode".into(), json!("subagent"));
        review.insert(
            "description".into(),
            json!("Reviews diffs and test failures. Read-only."),
        );
        review.insert(
            "permission".into(),
            json!({ "edit": "deny", "bash": bash_rules("deny", BASH_EXPLORE_ALLOW.iter().map(|s| (*s).to_owned())), "webfetch": "deny" }),
        );
        agents.insert("review".into(), Value::Object(review));
    }
    if let Some((slot, m)) = deep_slot {
        // §18: the deep model analyses and proposes; it never edits code in the first stage.
        let mut deep = agent_entry(Some(model_ref(slot, m)), Some(m));
        deep.insert("mode".into(), json!("subagent"));
        deep.insert(
            "description".into(),
            json!(
                "Deep reasoning for persistent failures and hard architecture questions. \
                   Analyses and proposes; never edits."
            ),
        );
        deep.insert(
            "permission".into(),
            json!({ "edit": "deny", "bash": bash_rules("deny", BASH_EXPLORE_ALLOW.iter().map(|s| (*s).to_owned())), "webfetch": "deny" }),
        );
        agents.insert("deep".into(), Value::Object(deep));
    }

    let mut mcp = Map::new();
    mcp.insert(
        identity::MCP_SERVER_NAME.into(),
        json!({
            "type": "local",
            "command": [agent_bin.to_string_lossy(), "mcp"],
            "enabled": true,
            "timeout": mcp_timeout_ms(cfg),
        }),
    );

    let mut root = Map::new();
    root.insert("$schema".into(), json!("https://opencode.ai/config.json"));
    if let Some(r) = &main_ref {
        root.insert("model".into(), json!(r));
        // Title/summary generation must stay local too, or OpenCode picks a hosted small model.
        root.insert("small_model".into(), json!(r));
    }
    root.insert(
        "enabled_providers".into(),
        json!(providers.keys().cloned().collect::<Vec<_>>()),
    );
    root.insert("share".into(), json!("disabled"));
    root.insert("autoupdate".into(), json!(false));
    root.insert("provider".into(), Value::Object(providers));
    root.insert("agent".into(), Value::Object(agents));
    root.insert("mcp".into(), Value::Object(mcp));
    Value::Object(root)
}

const EXPLORE_PROMPT: &str = "You are a read-only exploration subagent. Gather evidence to answer \
the parent's question using search, read and the local-agent tools. Do not edit files. Answer with \
a compact JSON object: {\"summary\": str, \"findings\": [str], \"evidence\": [str], \"confidence\": \
number, \"recommendation\": str, \"files\": [str], \"commands\": [str], \"unresolved\": [str]}. \
Cite files as path:line. Never paste whole files.";

// ---------------------------------------------------------------------------------------------
// Context transform (M6): the per-request view of completed tool results
// ---------------------------------------------------------------------------------------------

/// Built-in OpenCode tools that only read.
const READ_ONLY_BUILTINS: [&str; 5] = ["read", "list", "glob", "grep", "codesearch"];
/// Harness tools that only read, by name without the MCP prefix.
const READ_ONLY_HARNESS: [&str; 15] = [
    "git_status",
    "git_diff",
    "git_history",
    "git_log_range",
    "repo_map",
    "file_summary",
    "symbol_search",
    "find_references",
    "dependency_search",
    "inspect_config",
    "harness_info",
    "inspect_artifact",
    "summarize_experiment",
    "compare_experiments",
    "parse_test_failure",
];

/// A completed tool result in a request: where it sits, and the output the model would see.
struct ToolResult {
    message: usize,
    part: usize,
    call_id: String,
    tool: String,
    verification: bool,
    read_only: bool,
    output: String,
}

/// The name of a harness tool without its MCP prefix; `None` for a built-in tool.
fn harness_name(tool: &str) -> Option<&str> {
    let prefix = format!("{}_", identity::MCP_SERVER_NAME);
    tool.strip_prefix(prefix.as_str())
}

/// Whether a tool only reads. Bash is never read-only here: the policy classifies each command, and this rule
/// only ever drops results that cannot have changed anything.
fn is_read_only(tool: &str) -> bool {
    match harness_name(tool) {
        Some(name) => READ_ONLY_HARNESS.contains(&name),
        None => READ_ONLY_BUILTINS.contains(&tool),
    }
}

/// The completed tool results of a request, oldest first, and the size of its text: text and reasoning parts,
/// and the input and output of every tool part.
fn scan_request(messages: &[Value]) -> (Vec<ToolResult>, usize) {
    let mut results = Vec::new();
    let mut total = 0;
    for (message_index, message) in messages.iter().enumerate() {
        let Some(parts) = message["parts"].as_array() else {
            continue;
        };
        for (part_index, part) in parts.iter().enumerate() {
            match part["type"].as_str() {
                Some("text" | "reasoning") => {
                    total += part["text"].as_str().map_or(0, str::len);
                }
                Some("tool") => {
                    let state = &part["state"];
                    total += state["input"].to_string().len();
                    if state["status"] != "completed" {
                        continue;
                    }
                    let output = state["output"].as_str().unwrap_or_default();
                    total += output.len();
                    let tool = part["tool"].as_str().unwrap_or_default();
                    let short = harness_name(tool).unwrap_or(tool);
                    let command = state["input"]["command"].as_str();
                    results.push(ToolResult {
                        message: message_index,
                        part: part_index,
                        call_id: part["callID"].as_str().unwrap_or_default().to_owned(),
                        tool: tool.to_owned(),
                        verification: is_verification_tool(short)
                            || (short == "bash" && is_test_command(command)),
                        read_only: is_read_only(tool),
                        output: output.to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    (results, total)
}

/// `experimental.chat.messages.transform` (M6): decides, for this request only, what each completed tool result
/// looks like. Returns only the outputs that change, as `{message, part, output}`; the shim applies them to its
/// copy of the messages. The stored session is not edited, so the next request starts from the full history.
/// Compaction is not a security control: outputs are redacted in `tool.execute.after`, before they are stored.
pub fn messages_transform(input: &Value, ctx: &HookContext<'_>) -> (Value, Option<Event>) {
    let messages = input["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (results, total) = scan_request(messages);
    let views: Vec<ResultView> = results
        .iter()
        .map(|result| ResultView {
            call_id: result.call_id.clone(),
            tool: result.tool.clone(),
            bytes: result.output.len(),
            verification: result.verification,
            read_only: result.read_only,
        })
        .collect();
    let settings = &ctx.config.context;
    let budget = context::Budget::from_tokens(settings.target_tokens, settings.hard_limit);
    let actions = if settings.compact_tool_outputs {
        context::plan(&views, total, budget)
    } else {
        vec![context::Action::Keep; views.len()]
    };

    let mut changes = Vec::new();
    let mut bytes_after = total;
    for (result, action) in results.iter().zip(actions) {
        let replacement = match action {
            context::Action::Keep => continue,
            context::Action::Compact if result.verification => {
                context::compact_verification(&result.output)
            }
            context::Action::Compact => context::compact_text(&result.output),
            context::Action::Drop => {
                context::stub(harness_name(&result.tool).unwrap_or(result.tool.as_str()))
            }
        };
        if replacement.len() >= result.output.len() {
            continue;
        }
        bytes_after = bytes_after.saturating_sub(result.output.len() - replacement.len());
        changes.push(json!({
            "message": result.message,
            "part": result.part,
            "output": replacement,
        }));
    }

    let mut ev = Event::now(EventKind::Context);
    ev.status = Some(
        if changes.is_empty() {
            "unchanged"
        } else {
            "changed"
        }
        .to_owned(),
    );
    let ev = ev
        .with("tool_results", results.len())
        .with("changed", changes.len())
        .with("bytes_before", total)
        .with("bytes_after", bytes_after)
        .with("target_bytes", budget.target_bytes)
        .with("hard_bytes", budget.hard_bytes);
    (json!({ "changes": changes }), Some(ev))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod context_tests {
    use super::*;
    use harness_core::config::Config;
    use harness_core::security::{redact::Redactor, tool_policy::ToolPolicy};

    /// One assistant message with one completed tool part.
    fn tool_message(tool: &str, command: &str, output: &str) -> Value {
        json!({
            "info": { "role": "assistant" },
            "parts": [{
                "type": "tool",
                "callID": "call",
                "tool": tool,
                "state": {
                    "status": "completed",
                    "input": { "command": command },
                    "output": output,
                    "title": "",
                    "metadata": {},
                    "time": { "start": 0, "end": 1 }
                }
            }]
        })
    }

    fn transform(config: &Config, messages: Vec<Value>) -> (Value, Option<Event>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = ToolPolicy::with_rules(dir.path(), &config.security, false).expect("policy");
        let redactor = Redactor::new();
        let ctx = HookContext {
            config,
            policy: &policy,
            redactor: &redactor,
        };
        messages_transform(&json!({ "messages": messages }), &ctx)
    }

    #[test]
    fn older_reads_are_dropped_over_the_hard_limit_and_the_recent_ones_are_kept() {
        let mut config = Config::default();
        config.context.target_tokens = 1_000;
        config.context.hard_limit = 500;
        let big = "line of output\n".repeat(400);
        let messages: Vec<Value> = (0..5).map(|_| tool_message("read", "", &big)).collect();

        let (answer, event) = transform(&config, messages);

        let changes = answer["changes"].as_array().expect("changes");
        // The two oldest reads are dropped; the recent ones are compacted too, because they would not fit.
        assert!(changes.len() >= 2, "{answer}");
        assert_eq!(changes[0]["message"], 0);
        assert!(
            changes[0]["output"]
                .as_str()
                .unwrap()
                .starts_with("[dropped by the harness")
        );
        let event = event.expect("one record per request");
        assert_eq!(event.status.as_deref(), Some("changed"));
        let before = event.data.get("bytes_before").and_then(Value::as_u64);
        let after = event.data.get("bytes_after").and_then(Value::as_u64);
        assert!(
            after < before,
            "the request got smaller: {before:?} to {after:?}"
        );
        assert!(
            after <= Some(2_000),
            "the request fits the hard limit: {after:?}"
        );
    }

    #[test]
    fn an_old_verification_keeps_only_its_failure_lines() {
        let config = Config::default();
        let output = format!(
            "{}FAILED tests/test_calc.py::test_add - assert 1 == 2\nok\n",
            "....\n".repeat(300)
        );
        let mut messages = vec![tool_message("local-agent_run_tests", "", &output)];
        messages.extend((0..3).map(|_| tool_message("local-agent_git_status", "", "clean")));

        let (answer, _) = transform(&config, messages);

        let changes = answer["changes"].as_array().expect("changes");
        assert_eq!(changes.len(), 1, "{answer}");
        let compacted = changes[0]["output"].as_str().unwrap();
        assert!(compacted.contains("FAILED tests/test_calc.py::test_add"));
        assert!(!compacted.contains("...."));
    }

    #[test]
    fn with_compaction_off_the_request_is_unchanged_but_still_measured() {
        let mut config = Config::default();
        config.context.compact_tool_outputs = false;
        config.context.target_tokens = 100;
        config.context.hard_limit = 100;
        let big = "x\n".repeat(3000);
        let messages: Vec<Value> = (0..5).map(|_| tool_message("read", "", &big)).collect();

        let (answer, event) = transform(&config, messages);

        assert_eq!(answer["changes"], json!([]));
        assert_eq!(
            event.expect("one record").status.as_deref(),
            Some("unchanged")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_semantics() {
        assert!(wildcard_match("git status*", "git status --short"));
        assert!(wildcard_match("pytest*", "pytest && rm -rf ~"));
        assert!(wildcard_match("pwd", "pwd"));
        assert!(!wildcard_match("pwd", "pwd; rm x"));
        assert!(wildcard_match("cat *", "cat a.txt"));
        assert!(!wildcard_match("cat *", "concat"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn maps_builtin_tools() {
        let r = map_tool_call("read", &json!({"filePath": "src/a.rs"}));
        assert_eq!(r.kind, ToolKind::Read);
        assert_eq!(r.paths, vec!["src/a.rs"]);

        let r = map_tool_call("bash", &json!({"command": "ls", "workdir": "sub"}));
        assert_eq!(r.kind, ToolKind::Shell);
        assert_eq!(r.command.as_deref(), Some("ls"));
        assert_eq!(r.workdir.as_deref(), Some("sub"));

        let r = map_tool_call("local-agent_git_status", &json!({}));
        assert_eq!(r.kind, ToolKind::Harness("git_status".into()));

        let r = map_tool_call("mystery", &json!({}));
        assert_eq!(r.kind, ToolKind::Other("mystery".into()));
    }

    #[test]
    fn glob_patterns_that_escape_are_checked() {
        // The literal base is checked as a path; the whole pattern is checked for secret names (M9).
        let r = map_tool_call("glob", &json!({"pattern": "../../**/*.env"}));
        assert_eq!(r.paths, vec!["../..", "../../**/*.env"]);
        // No escaping base; the pattern itself is still judged for secret names.
        let r = map_tool_call("glob", &json!({"pattern": "src/**/*.rs"}));
        assert_eq!(r.paths, vec!["src/**/*.rs"]);
        let r = map_tool_call("grep", &json!({"pattern": "fn main", "path": "/etc"}));
        assert_eq!(r.paths, vec!["/etc"]);
    }

    #[test]
    fn patch_envelope_paths() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: ../evil.sh\n+rm\n*** End Patch";
        let r = map_tool_call("apply_patch", &json!({"patchText": patch}));
        assert_eq!(r.kind, ToolKind::Patch);
        assert_eq!(r.paths, vec!["src/a.rs", "../evil.sh"]);
    }

    #[test]
    fn permission_mapping() {
        let r = map_permission(
            &json!({"type": "bash", "pattern": "git push", "metadata": {"command": "git push --force"}}),
        );
        assert_eq!(r.kind, ToolKind::Shell);
        assert_eq!(r.command.as_deref(), Some("git push --force"));
        let r = map_permission(
            &json!({"type": "external_directory", "pattern": ["/etc/*"], "metadata": {"filepath": "/etc/passwd"}}),
        );
        assert_eq!(r.paths, vec!["/etc/passwd"]);
        let r = map_permission(
            &json!({"type": "edit", "pattern": "src/a.rs", "metadata": {"filePath": "src/a.rs"}}),
        );
        assert_eq!(r.kind, ToolKind::Edit);
        assert_eq!(r.paths, vec!["src/a.rs"]);
    }

    #[test]
    fn shell_env_blanks_secrets_only() {
        let vars = vec![
            ("OPENAI_API_KEY".to_owned(), "sk-x".to_owned()),
            ("PATH".to_owned(), "/bin".to_owned()),
            ("GITHUB_TOKEN".to_owned(), "ghp".to_owned()),
        ];
        let v = shell_env(vars.into_iter());
        let set = v["set"].as_object().expect("object");
        assert!(set.contains_key("OPENAI_API_KEY"));
        assert!(set.contains_key("GITHUB_TOKEN"));
        assert!(!set.contains_key("PATH"));
    }

    #[test]
    fn ask_verdicts_are_blocked_not_allowed() {
        use harness_core::config::Config;
        use harness_core::security::{redact::Redactor, tool_policy::ToolPolicy};

        let dir = tempfile::tempdir().expect("tempdir");
        let config = Config::default();
        let policy = ToolPolicy::with_rules(dir.path(), &config.security, false).expect("policy");
        let redactor = Redactor::new();
        let ctx = HookContext {
            config: &config,
            policy: &policy,
            redactor: &redactor,
        };
        // An unrecognised program is EXECUTE_RISKY, whose default verdict is Ask.
        let input = json!({
            "tool": "bash",
            "args": { "command": "some-unknown-tool --flag" },
            "sessionID": "s",
            "callID": "c"
        });
        let (answer, _) = handle(HookEvent::ToolBefore, &input, &ctx);
        assert_eq!(answer["action"], "block");
    }

    #[test]
    fn failure_answers_fail_closed() {
        assert_eq!(
            failure_answer(HookEvent::ToolBefore, "x")["action"],
            "block"
        );
        assert_eq!(failure_answer(HookEvent::Permission, "x")["status"], "deny");
        assert_eq!(failure_answer(HookEvent::Event, "x"), json!({}));
    }

    #[test]
    fn shell_env_blanks_every_secret_name_shape() {
        let secret_names = [
            "ENCRYPTION_KEY",
            "PASSPHRASE",
            "SESSION_ID",
            "SIGNING_KEY",
            "ACCESS_TOKENS",
            "CLIENT_SECRET",
            "x-api-key",
        ];
        let mut vars: Vec<(String, String)> = secret_names
            .iter()
            .map(|name| ((*name).to_owned(), "hunter2".to_owned()))
            .collect();
        vars.push(("MAX_TOKENS".to_owned(), "4096".to_owned()));
        let v = shell_env(vars.into_iter());
        let set = v["set"].as_object().expect("object");
        for name in secret_names {
            assert!(set.contains_key(name), "{name} should be blanked");
        }
        assert!(!set.contains_key("MAX_TOKENS"));
    }

    #[test]
    fn blocked_message_and_event_do_not_echo_secrets() {
        use harness_core::config::Config;
        use harness_core::security::{redact::Redactor, tool_policy::ToolPolicy};

        let dir = tempfile::tempdir().expect("tempdir");
        let config = Config::default();
        let policy = ToolPolicy::with_rules(dir.path(), &config.security, false).expect("policy");
        let redactor = Redactor::new();
        let ctx = HookContext {
            config: &config,
            policy: &policy,
            redactor: &redactor,
        };
        // The classifier's reason names the unrecognised program, which here is a key.
        let input = json!({
            "tool": "bash",
            "args": { "command": "sk-proj-AbCdEf0123456789xyz --now" },
            "sessionID": "s",
            "callID": "c"
        });
        let (answer, event) = handle(HookEvent::ToolBefore, &input, &ctx);
        assert_eq!(answer["action"], "block");
        // The classifier lower-cases the program name, so compare case-insensitively.
        let key = "sk-proj-abcdef0123456789xyz";
        let message = answer["message"].as_str().expect("message");
        assert!(!message.to_ascii_lowercase().contains(key), "{message}");
        let logged = serde_json::to_string(&event)
            .expect("event")
            .to_ascii_lowercase();
        assert!(!logged.contains(key), "{logged}");
    }

    #[test]
    fn mcp_timeout_covers_every_verification_step_in_turn() {
        let cfg = harness_core::config::from_toml_str(
            r#"
            [verification]
            default_timeout_ms = 600000
            [[verification.steps]]
            command = "python -m pytest -q"
            [[verification.steps]]
            command = "ruff check ."
            timeout_ms = 60000
            "#,
        )
        .expect("config");
        // 600 s + 60 s of steps, plus 30 s of margin; OpenCode's 10 s default would cut it short.
        assert_eq!(mcp_timeout_ms(&cfg), 690_000);
    }

    #[test]
    fn generated_config_is_local_first() {
        let cfg = harness_core::config::from_toml_str(
            r#"
            [models.main]
            provider = "llama_cpp"
            id = "qwen3.5-9b"
            role = "engineer"
            endpoint = "http://127.0.0.1:8001/v1"
            context = 16384
            hardware = "gpu"
            capabilities = { tool_calling = true }
            [models.deep]
            provider = "prism_bonsai"
            id = "bonsai-2-27b"
            role = "deep_reasoning"
            endpoint = "http://127.0.0.1:8003/v1"
            context = 24576
            hardware = "gpu"
            "#,
        )
        .expect("config");
        let oc = generate_opencode_config(&cfg, Path::new("/opt/agent"));
        assert_eq!(oc["share"], "disabled");
        assert_eq!(oc["autoupdate"], false);
        assert_eq!(oc["model"], "local-agent-main/qwen3.5-9b");
        assert_eq!(oc["small_model"], "local-agent-main/qwen3.5-9b");
        assert_eq!(
            oc["provider"]["local-agent-main"]["options"]["baseURL"],
            "http://127.0.0.1:8001/v1"
        );
        assert_eq!(oc["mcp"]["local-agent"]["command"][1], "mcp");
        // No verification steps: the git tools set the floor (4 × 30 s) plus the margin.
        assert_eq!(oc["mcp"]["local-agent"]["timeout"], 150_000);
        assert_eq!(oc["agent"]["deep"]["permission"]["edit"], "deny");
        assert_eq!(oc["agent"]["explore"]["permission"]["edit"], "deny");
        assert_eq!(oc["agent"]["build"]["permission"]["bash"]["*"], "ask");
        let enabled = oc["enabled_providers"].as_array().expect("array");
        assert_eq!(enabled.len(), 2);
        // Catch-all must be the first bash rule (later rules override earlier ones).
        let first = oc["agent"]["build"]["permission"]["bash"]
            .as_object()
            .and_then(|m| m.keys().next().cloned());
        assert_eq!(first.as_deref(), Some("*"));
    }
}
