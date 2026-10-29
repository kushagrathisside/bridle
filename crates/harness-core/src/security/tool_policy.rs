//! Tool policy: one allow / ask / deny verdict per tool request.
//!
//! The policy is host-neutral. Host adapters translate their tool calls (OpenCode's `read`,
//! `bash`, ...) into a [`ToolRequest`] (a [`ToolKind`], the file paths it touches and the shell
//! command it runs) and then ask [`ToolPolicy::evaluate`]. The policy inspects the real arguments;
//! a model cannot talk its way past it.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ToolClass;
use super::command_risk::{CommandAssessment, ShellKind, argument_words_in, assess_command};
use super::path_guard::{PathGuard, PathViolation, has_percent_expansion, is_unresolvable};
use super::redact::{is_secret_file, is_sensitive_entry};
use crate::config::{SecuritySection, Verdict};

/// Host-neutral tool families.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    List,
    Search,
    Edit,
    Write,
    Patch,
    Shell,
    WebFetch,
    WebSearch,
    Subagent,
    Todo,
    /// One of this harness's own MCP tools, by bare tool name (e.g. `git_status`).
    Harness(String),
    /// Anything the adapter does not recognise.
    Other(String),
}

/// A normalised tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub kind: ToolKind,
    /// File-system paths the call reads or writes, as given by the model.
    pub paths: Vec<String>,
    /// Shell command text for [`ToolKind::Shell`].
    pub command: Option<String>,
    /// Working directory override for shell commands, if the call specifies one.
    pub workdir: Option<String>,
}

impl ToolRequest {
    pub fn new(kind: ToolKind) -> Self {
        Self {
            kind,
            paths: Vec::new(),
            command: None,
            workdir: None,
        }
    }
}

/// The policy's answer, with enough detail for `agent explain` and the event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub verdict: Verdict,
    pub class: ToolClass,
    /// Short stable identifier of the rule that decided, e.g. `path_guard`, `command_risk`.
    pub rule: String,
    pub reasons: Vec<String>,
}

impl PolicyDecision {
    fn new(verdict: Verdict, class: ToolClass, rule: &str, reason: impl Into<String>) -> Self {
        Self {
            verdict,
            class,
            rule: rule.to_owned(),
            reasons: vec![reason.into()],
        }
    }
}

/// Harness MCP tools that only read state.
const READ_ONLY_HARNESS_TOOLS: &[&str] = &[
    "git_status",
    "git_diff",
    "git_history",
    "git_log_range",
    "parse_test_failure",
    "inspect_config",
    "repo_map",
    "file_summary",
    "symbol_search",
    "find_references",
    "dependency_search",
    "inspect_artifact",
    "summarize_experiment",
    "compare_experiments",
    "harness_info",
];
/// Harness and host control files, relative to the workspace root (lowercase). The harness reads
/// its profile, and the host its config, on every call, so the agent must not change them.
const PROTECTED_ROOT_ENTRIES: &[&str] = &[
    "local-agent.toml",
    "opencode.json",
    "opencode.jsonc",
    ".opencode",
    ".local-agent",
    ".worktrees",
    GIT_DIR,
];
/// The git metadata name. `core.fsmonitor` and hooks in it run code on later git calls, so it is
/// protected at any depth, not only at the root.
const GIT_DIR: &str = ".git";
/// Harness MCP tools that execute configured commands; each command is policy-checked inside the
/// tool itself, so the call is classed EXECUTE_SAFE here.
const EXECUTING_HARNESS_TOOLS: &[&str] = &[
    "run_verification",
    "run_tests",
    "run_linter",
    "run_typechecker",
    "run_command",
];

#[derive(Debug, Clone)]
pub struct ToolPolicy {
    security: SecuritySection,
    guard: PathGuard,
    workspace: PathBuf,
    shell: ShellKind,
}

impl ToolPolicy {
    /// Builds a policy for `workspace` plus `security.allowed_paths`.
    pub fn new(workspace: &Path, security: &SecuritySection) -> std::io::Result<Self> {
        Self::with_rules(workspace, security, cfg!(windows))
    }

    /// Like [`ToolPolicy::new`] with explicit Windows path rules (for cross-platform tests).
    pub fn with_rules(
        workspace: &Path,
        security: &SecuritySection,
        windows_rules: bool,
    ) -> std::io::Result<Self> {
        let mut roots = vec![workspace.to_path_buf()];
        roots.extend(security.allowed_paths.iter().cloned());
        let guard = PathGuard::new(roots, windows_rules)?;
        let workspace = guard
            .roots()
            .first()
            .cloned()
            .unwrap_or_else(|| workspace.to_path_buf());
        Ok(Self {
            security: security.clone(),
            guard,
            workspace,
            shell: ShellKind::host_default(),
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Maps a tool class to the configured verdict.
    pub fn class_verdict(&self, class: ToolClass) -> Verdict {
        match class {
            ToolClass::ReadOnly | ToolClass::Write | ToolClass::ExecuteSafe => Verdict::Allow,
            ToolClass::ExecuteRisky => self.security.risky_commands,
            ToolClass::Network => self.security.network,
            ToolClass::Destructive => self.security.destructive_commands,
            ToolClass::Privileged => self.security.privileged_commands,
        }
    }

    /// Classifies a shell command with the configured extra safe prefixes, in the host's shell.
    pub fn assess(&self, command: &str) -> CommandAssessment {
        self.assess_in(command, self.shell)
    }

    /// [`Self::assess`] for a command the harness runs itself in a known shell (verification
    /// steps), which must be classified in the grammar that will run it.
    pub fn assess_in(&self, command: &str, shell: ShellKind) -> CommandAssessment {
        assess_command(command, shell, &self.security.safe_commands)
    }

    /// Denies a shell command whose arguments name a secret file, a glob that could expand to one,
    /// or a path outside the authorised roots. Returns `None` when every argument passes.
    fn shell_argument_violation(&self, command: &str, base: &Path) -> Option<PolicyDecision> {
        for word in argument_words_in(command, self.shell) {
            // A word too large to expand is left to `unresolved_argument`.
            for candidate in argument_candidates(&word).unwrap_or_default() {
                if let Some(decision) = self.argument_violation(&candidate, base) {
                    return Some(decision);
                }
            }
        }
        None
    }

    /// The secret and path checks for one argument text (see [`argument_candidates`]).
    fn argument_violation(&self, word: &str, base: &Path) -> Option<PolicyDecision> {
        if is_null_device(word) {
            return None;
        }
        if may_name_secret(word) || glob_may_match_secret(word) {
            return Some(PolicyDecision::new(
                Verdict::Deny,
                ToolClass::ReadOnly,
                "secret_file",
                format!("`{word}` may name credentials; shell access to it is denied"),
            ));
        }
        if looks_like_path(word)
            && let Err(v) = self.guard.check(Path::new(word), base)
        {
            return Some(guard_denial(word, &v));
        }
        None
    }

    /// Denies a shell argument that only the shell can resolve at run time (`~`, `$HOME`, `$(...)`,
    /// a backtick, `%NAME%` in a path). Such a word is checked only when the command would
    /// otherwise run: a command already classed Ask or Deny keeps its own rule.
    fn unresolved_argument(&self, command: &str) -> Option<PolicyDecision> {
        for word in argument_words_in(command, self.shell) {
            let Some(candidates) = argument_candidates(&word) else {
                return Some(unresolved_denial(&word));
            };
            if let Some(candidate) = candidates
                .iter()
                .find(|c| !is_null_device(c) && is_unresolved_argument(c))
            {
                return Some(unresolved_denial(candidate));
            }
        }
        None
    }

    /// Whether a resolved path is a harness or host control file: a root-level entry from
    /// [`PROTECTED_ROOT_ENTRIES`] inside the workspace, or any `.git` file or directory below it.
    fn is_protected(&self, resolved: &Path) -> bool {
        let Ok(relative) = resolved.strip_prefix(&self.workspace) else {
            return false;
        };
        let fold = |name: &str| {
            if self.guard.windows_rules() {
                name.to_ascii_lowercase()
            } else {
                name.to_owned()
            }
        };
        let names: Vec<String> = relative
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => name.to_str().map(fold),
                _ => None,
            })
            .collect();
        let root_entry = names
            .first()
            .is_some_and(|first| PROTECTED_ROOT_ENTRIES.contains(&first.as_str()));
        root_entry || names.iter().any(|name| name == GIT_DIR)
    }

    /// The first argument or redirect target of `command` that names a protected control file,
    /// resolved against `base` the way the shell would resolve it.
    fn shell_touches_protected(&self, command: &str, base: &Path) -> Option<String> {
        for word in argument_words_in(command, self.shell) {
            for candidate in argument_candidates(&word).unwrap_or_default() {
                if let Ok(resolved) = self.guard.check(Path::new(&candidate), base)
                    && self.is_protected(&resolved)
                {
                    return Some(candidate);
                }
            }
        }
        None
    }

    pub fn evaluate(&self, req: &ToolRequest) -> PolicyDecision {
        // 1. Secret files never enter model context; writing them needs a human.
        let secret = req.paths.iter().find(|p| may_name_secret(p));
        if let Some(p) = secret {
            match req.kind {
                // Harness tools only read what they are pointed at, and none takes a secret file.
                ToolKind::Read | ToolKind::List | ToolKind::Search | ToolKind::Harness(_) => {
                    return PolicyDecision::new(
                        Verdict::Deny,
                        ToolClass::ReadOnly,
                        "secret_file",
                        format!("{p} may contain credentials and is never sent to a model"),
                    );
                }
                ToolKind::Edit | ToolKind::Write | ToolKind::Patch => {
                    return PolicyDecision::new(
                        Verdict::Ask,
                        ToolClass::Write,
                        "secret_file",
                        format!("{p} may contain credentials; editing it needs approval"),
                    );
                }
                _ => {}
            }
        }

        // 2. Every path must stay inside the authorised roots, whatever the tool.
        let base = match &req.workdir {
            Some(w) => match self.guard.check(Path::new(w), &self.workspace) {
                Ok(p) => p,
                Err(v) => {
                    return PolicyDecision::new(
                        Verdict::Deny,
                        ToolClass::Privileged,
                        path_rule(&v),
                        format!("working directory rejected: {v}"),
                    );
                }
            },
            None => self.workspace.clone(),
        };
        for p in &req.paths {
            match self.guard.check(Path::new(p), &base) {
                Err(v) => return guard_denial(p, &v),
                Ok(resolved) => {
                    if matches!(req.kind, ToolKind::Edit | ToolKind::Write | ToolKind::Patch)
                        && self.is_protected(&resolved)
                    {
                        return protected_denial(p);
                    }
                }
            }
        }

        // A file read names no file: it is malformed, and fail-closed says deny it.
        if req.kind == ToolKind::Read && req.paths.is_empty() {
            return PolicyDecision::new(
                Verdict::Deny,
                ToolClass::ReadOnly,
                "malformed",
                "read call without a file path",
            );
        }

        // 3. Classify by tool family.
        match &req.kind {
            ToolKind::Read
            | ToolKind::List
            | ToolKind::Search
            | ToolKind::Subagent
            | ToolKind::Todo => PolicyDecision::new(
                Verdict::Allow,
                ToolClass::ReadOnly,
                "tool_kind",
                "read-only tool",
            ),
            ToolKind::Edit | ToolKind::Write | ToolKind::Patch => PolicyDecision::new(
                Verdict::Allow,
                ToolClass::Write,
                "tool_kind",
                "file edit inside the workspace",
            ),
            ToolKind::WebFetch | ToolKind::WebSearch => PolicyDecision::new(
                self.class_verdict(ToolClass::Network),
                ToolClass::Network,
                "tool_kind",
                "network access",
            ),
            ToolKind::Shell => {
                let Some(cmd) = req.command.as_deref() else {
                    return PolicyDecision::new(
                        Verdict::Deny,
                        ToolClass::ExecuteRisky,
                        "malformed",
                        "shell call without a command",
                    );
                };
                // Shell arguments are paths too. A command must not read a secret file, or
                // leave the workspace, just because the program is harmless (`cat .env`).
                if dumps_environment(cmd) {
                    return PolicyDecision::new(
                        Verdict::Deny,
                        ToolClass::ReadOnly,
                        "secret_env",
                        "the command prints the environment, which holds this machine's secrets; it is denied",
                    );
                }
                if let Some(decision) = self.shell_argument_violation(cmd, &base) {
                    return decision;
                }
                let a = self.assess(cmd);
                // Reads of a control file are fine; a command that can change something is not.
                if a.class > ToolClass::ReadOnly {
                    if self.is_protected(&base) {
                        return protected_denial(&base.display().to_string());
                    }
                    if let Some(word) = self.shell_touches_protected(cmd, &base) {
                        return protected_denial(&word);
                    }
                }
                let verdict = self.class_verdict(a.class);
                if verdict == Verdict::Allow
                    && let Some(decision) = self.unresolved_argument(cmd)
                {
                    return decision;
                }
                PolicyDecision {
                    verdict,
                    class: a.class,
                    rule: "command_risk".to_owned(),
                    reasons: a.reasons,
                }
            }
            ToolKind::Harness(name) if READ_ONLY_HARNESS_TOOLS.contains(&name.as_str()) => {
                PolicyDecision::new(
                    Verdict::Allow,
                    ToolClass::ReadOnly,
                    "harness_tool",
                    "read-only harness tool",
                )
            }
            ToolKind::Harness(name) if EXECUTING_HARNESS_TOOLS.contains(&name.as_str()) => {
                PolicyDecision::new(
                    Verdict::Allow,
                    ToolClass::ExecuteSafe,
                    "harness_tool",
                    "runs configured verification commands (each command is policy-checked)",
                )
            }
            // Configured adapters run their command through the same checks as run_command.
            ToolKind::Harness(name) if name.starts_with("adapter_") => PolicyDecision::new(
                self.class_verdict(ToolClass::ExecuteSafe),
                ToolClass::ExecuteSafe,
                "harness_tool",
                "configured adapter (its command is policy-checked inside the tool)",
            ),
            ToolKind::Harness(name) | ToolKind::Other(name) => PolicyDecision::new(
                self.class_verdict(ToolClass::ExecuteRisky),
                ToolClass::ExecuteRisky,
                "unknown_tool",
                format!("unrecognised tool `{name}`"),
            ),
        }
    }
}

/// The denial for a path the guard refused. An unresolvable path gets its own rule name.
fn guard_denial(word: &str, violation: &PathViolation) -> PolicyDecision {
    PolicyDecision::new(
        Verdict::Deny,
        ToolClass::Privileged,
        path_rule(violation),
        format!("{word}: {violation}"),
    )
}

fn protected_denial(word: &str) -> PolicyDecision {
    PolicyDecision::new(
        Verdict::Deny,
        ToolClass::Write,
        "protected_path",
        format!("{word}: harness and host control files are changed by the user, not the agent"),
    )
}

fn path_rule(violation: &PathViolation) -> &'static str {
    match violation {
        PathViolation::Unresolved => "unresolved_path",
        _ => "path_guard",
    }
}

/// Words the command resolves at run time. `command_risk` replaces `$(...)`, backticks and
/// `<(...)` with the `<subst>` marker, so a substitution is caught here as well.
fn is_unresolved_argument(word: &str) -> bool {
    is_unresolvable(word)
        || word.contains("<subst>")
        || (looks_like_path(word) && has_percent_expansion(word))
}

/// Null devices may be written to by shell redirection; they are not paths inside the workspace.
fn is_null_device(word: &str) -> bool {
    matches!(
        word,
        "/dev/null" | "NUL" | "nul" | "$null" | "$Null" | "$NULL"
    )
}

fn unresolved_denial(word: &str) -> PolicyDecision {
    PolicyDecision::new(
        Verdict::Deny,
        ToolClass::Privileged,
        "unresolved_path",
        format!("`{word}` cannot be resolved statically; shell access to it is denied"),
    )
}

/// The texts an argument can stand for. The word is brace-expanded (`.{env,netrc}`), and a
/// `--opt=value` option also contributes its value after the first `=`, which is a path when the
/// option names one (`--git-dir=/x/.git`). `None` when the expansion is too large to check.
fn argument_candidates(word: &str) -> Option<Vec<String>> {
    let mut texts = vec![word.to_owned()];
    // A bash word with escapes names the file the shell sees: `.en\v` is `.env`.
    let unescaped = unescape_shell_word(word);
    if unescaped != word {
        texts.push(unescaped);
    }
    if word.starts_with('-')
        && let Some((_, value)) = word.split_once('=')
        && !value.is_empty()
    {
        texts.push(value.to_owned());
    }
    let mut out = Vec::new();
    for text in texts {
        out.extend(expand_braces(&text)?);
    }
    Some(out)
}

/// The word the shell passes on after backslash escapes are removed (`.en\v` becomes `.env`).
fn unescape_shell_word(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(next) = chars.next()
        {
            out.push(next);
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a path, as named, may be a secret file: the secret names of `is_secret_file`, or a Windows short
/// (8.3) name such as `ENV~1`, which can address a file without naming it.
fn may_name_secret(path: &str) -> bool {
    // A `:` can separate a git revision from a path (`HEAD:.env`) or name an alternate stream, and
    // Windows drops trailing spaces and dots from a name. Each `:`-separated piece is judged with
    // those dropped, on every platform, so the verdict does not depend on the host the hook runs on.
    is_short_name(path)
        || path.split(':').any(|piece| {
            let named = piece.trim_end_matches([' ', '.']);
            is_secret_file(Path::new(named)) || glob_may_match_secret(named)
        })
}

/// Whether a path has a component with `~` followed by a digit, the form of a Windows short name.
fn is_short_name(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| {
        part.as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'~' && pair[1].is_ascii_digit())
    })
}

/// Whether a command prints the process environment, which holds the machine's keys (residual risk 4 in the security concepts page): `env`,
/// `printenv` or `set` with no operand, `export -p`, a PowerShell `Env:` drive listing, `printenv` of a secret-looking
/// name, or the .NET call that returns every variable. Each segment is judged alone, so a wrapper such as `env FOO=1 cmd`
/// is not a dump.
fn dumps_environment(command: &str) -> bool {
    if command
        .to_ascii_lowercase()
        .contains("getenvironmentvariables")
    {
        return true;
    }
    command
        .split([';', '|', '&', '\n'])
        .map(|segment| segment.split_whitespace().collect::<Vec<&str>>())
        .any(|words| env_dump(&words))
}

fn env_dump(words: &[&str]) -> bool {
    let Some(first) = words.first() else {
        return false;
    };
    let program = first
        .trim_matches(|c| c == '"' || c == '\'')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    if words
        .iter()
        .any(|w| w.to_ascii_lowercase().contains("env:"))
    {
        return true;
    }
    let operands: Vec<&str> = words[1..]
        .iter()
        .copied()
        .filter(|w| !w.starts_with('-') && !w.contains('='))
        .collect();
    match program.as_str() {
        // `set -e` is a shell option; a bare `set` lists the variables.
        "set" => words.len() == 1,
        "env" | "export" => operands.is_empty(),
        "printenv" => {
            operands.is_empty() || operands.iter().any(|name| is_sensitive_entry(name, ""))
        }
        _ => false,
    }
}

/// Expands bash brace alternatives into every word they name: `.{env,netrc}` gives `.env` and
/// `.netrc`. A group without a top-level comma is literal, as in the shell. `None` when the result
/// would be too large to check, which the caller treats as unresolvable.
fn expand_braces(word: &str) -> Option<Vec<String>> {
    const MAX_EXPANSIONS: usize = 256;
    let chars: Vec<char> = word.chars().collect();
    for open in (0..chars.len()).filter(|&i| chars[i] == '{') {
        let Some((close, commas)) = brace_group(&chars, open) else {
            continue;
        };
        if commas.is_empty() {
            continue;
        }
        let prefix: String = chars[..open].iter().collect();
        let suffix: String = chars[close + 1..].iter().collect();
        let mut out = Vec::new();
        let mut start = open + 1;
        for end in commas.iter().copied().chain(std::iter::once(close)) {
            let alternative: String = chars[start..end].iter().collect();
            // Expanding the result again handles later groups, including those in `suffix`.
            for expanded in expand_braces(&format!("{prefix}{alternative}{suffix}"))? {
                out.push(expanded);
                if out.len() > MAX_EXPANSIONS {
                    return None;
                }
            }
            start = end + 1;
        }
        return Some(out);
    }
    Some(vec![word.to_owned()])
}

/// The `}` closing the `{` at `open`, and the commas that separate its top-level alternatives.
fn brace_group(chars: &[char], open: usize) -> Option<(usize, Vec<usize>)> {
    let mut depth = 0usize;
    let mut commas = Vec::new();
    for (k, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((k, commas));
                }
            }
            ',' if depth == 1 => commas.push(k),
            _ => {}
        }
    }
    None
}

/// Heuristic: an argument that could name a file system location.
fn looks_like_path(word: &str) -> bool {
    !word.starts_with('-')
        && !is_url(word)
        && (word.contains('/')
            || word.contains('\\')
            || word.starts_with('.')
            || word.contains(':'))
}

/// `scheme://...` words are URLs, not file paths. On Windows the colon would otherwise read as a
/// drive or alternate-data-stream separator (`https:` ), so the network rule must decide instead.
fn is_url(word: &str) -> bool {
    let Some((scheme, rest)) = word.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        && !rest.is_empty()
}

/// One representative of every name rule in `is_secret_file`: exact names, suffixes, `.env.*`, and
/// the `.aws` and `.ssh` directory rules. A glob word is denied when it could match one, because
/// the shell expands `.e*` to `.env` and `.git-cred*` to `.git-credentials` before the program runs.
/// Each entry must be a secret by `is_secret_file`; `glob_prefixes_of_every_secret_name_are_denied`
/// checks that. The rule lists are private to `redact.rs`, so this mirror is maintained by hand.
const SECRET_SAMPLES: &[&str] = &[
    ".netrc",
    "_netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "cookies",
    "login data",
    "server.pem",
    "server.key",
    "server.p12",
    "server.pfx",
    "store.kdbx",
    ".env",
    ".env.local",
    ".aws/credentials",
    ".ssh/id_rsa",
];

/// Further names the glob check also protects. They are not secret by `is_secret_file`, but a
/// wildcard that reaches them is refused as it was before the samples were aligned with the rules.
const SECRET_EXTRA_SAMPLES: &[&str] = &["credentials", "secrets.json"];

/// Whether a word with wildcards could expand to one of the secret names. A word without `*`, `?`
/// or `[` is a literal, and `is_secret_file` judges it instead.
fn glob_may_match_secret(word: &str) -> bool {
    if !word.contains(['*', '?', '[']) {
        return false;
    }
    let pattern = parse_glob(word);
    // Name rules hold in any directory, so the last component of the word is matched against the
    // last component of each sample too (`keys/*.pem` against `server.pem`).
    let name = parse_glob(last_component(word));
    SECRET_SAMPLES
        .iter()
        .chain(SECRET_EXTRA_SAMPLES)
        .any(|sample| glob_matches(&pattern, sample) || glob_matches(&name, last_component(sample)))
}

/// The part of a path after its last separator, for either separator style.
fn last_component(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// One token of a shell glob.
#[derive(Debug, PartialEq, Eq)]
enum GlobToken {
    /// A literal character.
    Literal(char),
    /// `?`: any one character.
    AnyChar,
    /// `*`: any run of characters, including none.
    AnyRun,
    /// `[...]` or `[!...]`: one character from the inclusive ranges, or outside them if negated.
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl GlobToken {
    /// Whether this single-character token matches `c`, ignoring ASCII case like the secret rules.
    /// `AnyRun` spans several characters, so [`glob_matches`] handles it instead.
    fn matches(&self, c: char) -> bool {
        match self {
            Self::Literal(literal) => literal.eq_ignore_ascii_case(&c),
            Self::AnyChar => true,
            Self::AnyRun => false,
            Self::Class { negated, ranges } => {
                let hit = [c, c.to_ascii_lowercase(), c.to_ascii_uppercase()]
                    .into_iter()
                    .any(|x| ranges.iter().any(|&(lo, hi)| lo <= x && x <= hi));
                hit != *negated
            }
        }
    }
}

fn parse_glob(pattern: &str) -> Vec<GlobToken> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let (token, next) = match chars[i] {
            '*' => (GlobToken::AnyRun, i + 1),
            '?' => (GlobToken::AnyChar, i + 1),
            '[' => parse_class(&chars, i).unwrap_or((GlobToken::Literal('['), i + 1)),
            c => (GlobToken::Literal(c), i + 1),
        };
        tokens.push(token);
        i = next;
    }
    tokens
}

/// Parses the bracket expression opening at `open`. Returns it with the index after its `]`, or
/// `None` when it is never closed, in which case the `[` is an ordinary character as in the shell.
fn parse_class(chars: &[char], open: usize) -> Option<(GlobToken, usize)> {
    let mut k = open + 1;
    let negated = matches!(chars.get(k), Some('!' | '^'));
    if negated {
        k += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let c = *chars.get(k)?;
        // A `]` straight after the opening bracket (or its `!`) is a member, not the end.
        if c == ']' && !first {
            return Some((GlobToken::Class { negated, ranges }, k + 1));
        }
        first = false;
        match (chars.get(k + 1), chars.get(k + 2)) {
            (Some('-'), Some(&hi)) if hi != ']' => {
                ranges.push((c, hi));
                k += 3;
            }
            _ => {
                ranges.push((c, c));
                k += 1;
            }
        }
    }
}

/// Matches a parsed glob against `text` over whole strings. Dynamic programming over (token,
/// character) pairs keeps this linear in the pattern times the text, however many `*` appear.
fn glob_matches(pattern: &[GlobToken], text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let (n, m) = (pattern.len(), text.len());
    // `reach[i][j]`: pattern token `i` onwards matches the text from character `j` onwards.
    let mut reach = vec![vec![false; m + 1]; n + 1];
    reach[n][m] = true;
    for i in (0..n).rev() {
        for j in (0..=m).rev() {
            reach[i][j] = match pattern[i] {
                GlobToken::AnyRun => reach[i + 1][j] || (j < m && reach[i][j + 1]),
                _ => j < m && pattern[i].matches(text[j]) && reach[i + 1][j + 1],
            };
        }
    }
    reach[0][0]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn policy(dir: &Path) -> ToolPolicy {
        ToolPolicy::with_rules(dir, &SecuritySection::default(), false).expect("policy")
    }

    fn shell(command: &str) -> ToolRequest {
        let mut req = ToolRequest::new(ToolKind::Shell);
        req.command = Some(command.to_owned());
        req
    }

    fn read(path: &str) -> ToolRequest {
        let mut req = ToolRequest::new(ToolKind::Read);
        req.paths = vec![path.to_owned()];
        req
    }

    #[test]
    fn harness_history_tools_are_read_only_and_never_take_a_secret_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for tool in ["git_history", "git_log_range"] {
            let d = p.evaluate(&ToolRequest::new(ToolKind::Harness(tool.to_owned())));
            assert_eq!(
                (d.verdict, d.class),
                (Verdict::Allow, ToolClass::ReadOnly),
                "{tool}"
            );
        }
        let mut secret = ToolRequest::new(ToolKind::Harness("git_history".to_owned()));
        secret.paths = vec![".env".to_owned()];
        let d = p.evaluate(&secret);
        assert_eq!((d.verdict, d.rule.as_str()), (Verdict::Deny, "secret_file"));
    }

    #[test]
    fn escaped_words_are_judged_as_the_shell_reads_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_eq!(p.evaluate(&shell("cat .en\\v")).rule, "secret_file");
        assert_eq!(p.evaluate(&shell("cat \\.env")).rule, "secret_file");
    }

    #[test]
    fn short_name_aliases_of_a_file_are_denied_as_possible_secrets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        let d = p.evaluate(&read("ENV~1"));
        assert_eq!((d.verdict, d.rule.as_str()), (Verdict::Deny, "secret_file"));
        assert_eq!(p.evaluate(&shell("cat ENV~1")).rule, "secret_file");
        // A tilde without a digit is an ordinary name.
        assert_ne!(p.evaluate(&read("a~b.txt")).rule, "secret_file");
    }

    #[test]
    fn secret_names_are_denied_on_every_platform_whatever_the_host_drops_or_adds() {
        // Linux keeps a trailing space and treats `HEAD:.env` as a plain name; Windows drops the
        // space and refuses the colon. Both must be denied everywhere.
        for name in [
            ".env ",
            ".env.",
            "HEAD:.env",
            ":.env",
            "notes.txt:.env",
            "C:\\u\\.ssh\\id_rsa",
        ] {
            assert!(may_name_secret(name), "{name} must be denied");
        }
        for name in ["HEAD:README.md", "readme.txt ", "src/main.rs"] {
            assert!(!may_name_secret(name), "{name} must stay allowed");
        }
    }

    #[test]
    fn commands_that_dump_the_environment_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in [
            "env",
            "printenv",
            "set",
            "export -p",
            "env | grep PATH",
            "Get-ChildItem Env:",
            "gci env:",
            "[System.Environment]::GetEnvironmentVariables()",
            "printenv OPENAI_API_KEY",
            "echo $env:GITHUB_TOKEN",
        ] {
            assert_eq!(p.evaluate(&shell(cmd)).rule, "secret_env", "{cmd}");
        }
    }

    #[test]
    fn ordinary_uses_of_the_environment_are_not_dumps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in [
            "env FOO=1 git status",
            "printenv PATH",
            "set -e",
            "git status --short",
        ] {
            assert_ne!(p.evaluate(&shell(cmd)).rule, "secret_env", "{cmd}");
        }
    }

    #[test]
    fn shell_reads_of_secret_files_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in [
            "cat .env",
            "grep TOKEN .env.local",
            "type ./deploy/id_ed25519",
            "cat ~/.netrc",
        ] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            assert_eq!(d.rule, "secret_file", "{cmd}");
        }
    }

    #[test]
    fn globs_that_could_expand_to_a_secret_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_eq!(p.evaluate(&shell("cat .e*")).verdict, Verdict::Deny);
        assert_eq!(p.evaluate(&shell("grep x *.pem")).verdict, Verdict::Deny);
        // A glob that cannot match any secret sample is not denied by this rule.
        assert_ne!(p.evaluate(&shell("cat *.rs")).rule, "secret_file");
    }

    #[test]
    fn ordinary_reads_are_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_eq!(p.evaluate(&shell("cat README.md")).verdict, Verdict::Allow);
        assert_eq!(p.evaluate(&shell("ls src")).verdict, Verdict::Allow);
    }

    #[test]
    fn shell_paths_outside_the_workspace_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        let d = p.evaluate(&shell("cat ../outside.txt"));
        assert_eq!(d.verdict, Verdict::Deny);
        assert_eq!(d.rule, "path_guard");
    }

    #[test]
    fn null_device_redirection_is_not_a_path_violation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_ne!(p.evaluate(&shell("ls > /dev/null")).rule, "path_guard");
    }

    #[test]
    fn url_arguments_are_not_treated_as_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        // The network rule decides: the policy says Ask, and the hook turns Ask into a block.
        let d = p.evaluate(&shell("curl https://example.com"));
        assert_eq!(d.verdict, Verdict::Ask);
        assert_eq!(d.rule, "command_risk");
        assert!(super::is_url("https://example.com/x"));
        assert!(!super::is_url("C:\\data\\x"));
    }

    #[test]
    fn read_tool_on_a_secret_file_is_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        let d = p.evaluate(&read(".env"));
        assert_eq!(d.verdict, Verdict::Deny);
        assert_eq!(d.rule, "secret_file");
    }

    /// Item 13: `~` and `$VAR` are not expanded by the guard, so a word that starts with them (or
    /// contains a substitution) cannot be shown to stay in the workspace.
    #[test]
    fn tilde_and_variable_words_are_unresolved_not_joined_under_the_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in [
            "cat ~/.bashrc",
            "cat $HOME/notes.txt",
            "cat ${HOME}/x",
            "cd ~ && cat .bashrc",
            "echo x >> ~/.bashrc",
            "grep -r BEGIN ~",
            "cat $FILE",
            "cat \"$(printf '\\056env')\"",
            "cat $(printf '\\056env')",
            "cat %USERPROFILE%/.bashrc",
        ] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            // On Windows the inner `\056env` is a rooted path, so the workspace check can deny the
            // printf substitution before the unresolved-word check does. Either denial is right.
            let substitution = cmd.contains("$(printf");
            assert!(
                d.rule == "unresolved_path" || (substitution && d.rule == "path_guard"),
                "{cmd}: {}",
                d.rule
            );
        }
    }

    /// Punctuation that is not an expansion stays allowed: a regex anchor, a `%Y%m%d` date format.
    #[test]
    fn words_without_an_expansion_are_not_unresolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_eq!(
            p.evaluate(&shell("grep 'end$' README.md")).verdict,
            Verdict::Allow
        );
        assert_ne!(p.evaluate(&shell("date +%Y%m%d")).rule, "unresolved_path");
    }

    /// Item 13: a `cd` target is an argument like any other, so it is path-checked.
    #[test]
    fn cd_targets_are_path_checked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        let d = p.evaluate(&shell("cd /tmp && cat x"));
        assert_eq!(d.verdict, Verdict::Deny);
        assert_eq!(d.rule, "path_guard");
    }

    /// Item 14: `--opt=value` carries a path after the `=`, so the value is checked too.
    #[test]
    fn dash_options_with_an_equals_value_are_path_checked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for (cmd, rule) in [
            ("git --git-dir=/home/user/x/.git log", "path_guard"),
            ("git diff --output=/home/user/.bashrc", "path_guard"),
            ("python -c x --log-file=../../outside", "path_guard"),
            ("cat --file=.env", "secret_file"),
        ] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            assert_eq!(d.rule, rule, "{cmd}");
        }
        assert_ne!(
            p.evaluate(&shell("git --git-dir=.git log")).rule,
            "path_guard",
            "a workspace-relative value stays allowed"
        );
        assert_eq!(
            p.evaluate(&shell("git log --format=%h")).verdict,
            Verdict::Allow
        );
    }

    /// Item 15: bracket classes and brace alternatives name files too, so they are checked the
    /// same way `*` and `?` are.
    #[test]
    fn bracket_and_brace_patterns_are_checked_as_secret_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in ["cat .[e]nv", "cat .{env,netrc}", "cat .{x,env}"] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            assert_eq!(d.rule, "secret_file", "{cmd}");
        }
        assert_eq!(
            p.evaluate(&shell("cat .{README,notes}")).verdict,
            Verdict::Allow
        );
    }

    #[test]
    fn brace_and_bracket_expansion_edge_cases() {
        let words = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            expand_braces("a{b,c}d{e,f}"),
            Some(words(&["abde", "abdf", "acde", "acdf"]))
        );
        // Without a top-level comma the braces are literal, as in the shell.
        assert_eq!(expand_braces("{x}"), Some(words(&["{x}"])));
        assert_eq!(expand_braces("{a,b}"), Some(words(&["a", "b"])));
        // Too many alternatives to check: the caller treats the word as unresolvable.
        assert_eq!(expand_braces(&"{a,b}".repeat(9)), None);
        // `]` first in a class is a member; an unclosed `[` is an ordinary character.
        assert!(glob_matches(&parse_glob("[]x]"), "]"));
        assert!(!glob_may_match_secret("[.env"));
        assert!(glob_may_match_secret("[!x]env"));
    }

    /// Item 16: every name rule of `is_secret_file` must be reachable through a glob, so each
    /// sample is a secret and each of its prefixes followed by `*` is denied.
    #[test]
    fn glob_prefixes_of_every_secret_name_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for sample in SECRET_SAMPLES {
            assert!(
                is_secret_file(Path::new(sample)),
                "{sample} is a secret name"
            );
            for end in (1..=sample.len()).filter(|&n| sample.is_char_boundary(n)) {
                let glob = format!("{}*", &sample[..end]);
                assert!(glob_may_match_secret(&glob), "{glob} may name {sample}");
            }
        }
        for cmd in ["cat .git-cred*", "cat keys/*.pem", "cat .aws/cred*"] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            assert_eq!(d.rule, "secret_file", "{cmd}");
        }
        assert!(
            glob_may_match_secret("*.PEM"),
            "matching is case-insensitive"
        );
    }

    fn write(kind: ToolKind, path: &str) -> ToolRequest {
        let mut req = ToolRequest::new(kind);
        req.paths = vec![path.to_owned()];
        req
    }

    /// Item 17: the harness reads its profile and the host reads its config from these files on
    /// every call, so the agent must not write them, whatever path spelling it uses.
    #[test]
    fn harness_and_host_control_files_cannot_be_written_by_the_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for path in [
            "local-agent.toml",
            "opencode.json",
            "opencode.jsonc",
            ".opencode/plugin.ts",
            ".local-agent/state.db",
            ".git/config",
            ".git",
            "sub/../local-agent.toml",
            "sub/.git/hooks/pre-commit",
        ] {
            for kind in [ToolKind::Edit, ToolKind::Write, ToolKind::Patch] {
                let d = p.evaluate(&write(kind, path));
                assert_eq!(d.verdict, Verdict::Deny, "{path}");
                assert_eq!(d.rule, "protected_path", "{path}");
            }
        }
    }

    /// Item 17: under Windows naming rules the protected names are matched case-insensitively.
    #[test]
    fn protected_control_names_match_case_insensitively_under_windows_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p =
            ToolPolicy::with_rules(dir.path(), &SecuritySection::default(), true).expect("policy");
        for path in ["LOCAL-AGENT.TOML", ".GIT/config", "Opencode.JSON"] {
            let d = p.evaluate(&write(ToolKind::Write, path));
            assert_eq!(d.verdict, Verdict::Deny, "{path}");
            assert_eq!(d.rule, "protected_path", "{path}");
        }
        assert_eq!(
            p.evaluate(&write(ToolKind::Write, ".github/workflows/x.yml"))
                .verdict,
            Verdict::Allow
        );
    }

    /// Item 17: protection covers writes only. Reading the files and writing other files still
    /// works, including `.github`, which is not the `.git` directory.
    #[test]
    fn protected_files_stay_readable_and_other_files_stay_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        assert_eq!(p.evaluate(&read("opencode.json")).verdict, Verdict::Allow);
        for path in [".github/workflows/x.yml", "src/main.rs"] {
            assert_eq!(
                p.evaluate(&write(ToolKind::Write, path)).verdict,
                Verdict::Allow,
                "{path}"
            );
        }
        assert_eq!(
            p.evaluate(&shell("cat opencode.json")).verdict,
            Verdict::Allow
        );
        assert_eq!(p.evaluate(&shell("git log")).verdict, Verdict::Allow);
    }

    /// Item 17: a shell argument or redirect target that names a control file is a deny when the
    /// command can change something.
    #[test]
    fn shell_writes_to_protected_control_files_are_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = policy(dir.path());
        for cmd in [
            "echo x >> local-agent.toml",
            "sed -i s/a/b/ opencode.json",
            "cp x .git/hooks/pre-commit",
        ] {
            let d = p.evaluate(&shell(cmd));
            assert_eq!(d.verdict, Verdict::Deny, "{cmd}");
            assert_eq!(d.rule, "protected_path", "{cmd}");
        }
    }
}
