//! Tool policy (§25, §34): one allow / ask / deny verdict per tool request.
//!
//! The policy is host-neutral. Host adapters translate their tool calls (OpenCode's `read`,
//! `bash`, ...) into a [`ToolRequest`] (a [`ToolKind`], the file paths it touches and the shell
//! command it runs) and then ask [`ToolPolicy::evaluate`]. The policy inspects the real arguments;
//! a model cannot talk its way past it (R-25.2, R-59).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ToolClass;
use super::command_risk::{CommandAssessment, ShellKind, argument_words, assess_command};
use super::path_guard::{PathGuard, PathViolation, has_percent_expansion, is_unresolvable};
use super::redact::is_secret_file;
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

/// The policy's answer, with enough detail for `agent explain` and the event log (§54).
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
const READ_ONLY_HARNESS_TOOLS: &[&str] = &["git_status", "git_diff", "harness_info"];
/// Harness MCP tools that execute configured commands; each command is policy-checked inside the
/// tool itself, so the call is classed EXECUTE_SAFE here.
const EXECUTING_HARNESS_TOOLS: &[&str] = &["run_verification"];

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

    /// Classifies a shell command with the configured extra safe prefixes.
    pub fn assess(&self, command: &str) -> CommandAssessment {
        assess_command(command, self.shell, &self.security.safe_commands)
    }

    /// Denies a shell command whose arguments name a secret file, a glob that could expand to one,
    /// or a path outside the authorised roots. Returns `None` when every argument passes.
    fn shell_argument_violation(&self, command: &str, base: &Path) -> Option<PolicyDecision> {
        for word in argument_words(command) {
            if is_null_device(&word) {
                continue;
            }
            if is_secret_file(Path::new(&word)) || glob_may_match_secret(&word) {
                return Some(PolicyDecision::new(
                    Verdict::Deny,
                    ToolClass::ReadOnly,
                    "secret_file",
                    format!("`{word}` may name credentials; shell access to it is denied"),
                ));
            }
            if looks_like_path(&word)
                && let Err(v) = self.guard.check(Path::new(&word), base)
            {
                return Some(guard_denial(&word, &v));
            }
        }
        None
    }

    /// Denies a shell argument that only the shell can resolve at run time (`~`, `$HOME`, `$(...)`,
    /// a backtick, `%NAME%` in a path). Such a word is checked only when the command would
    /// otherwise run: a command already classed Ask or Deny keeps its own rule.
    fn unresolved_argument(&self, command: &str) -> Option<PolicyDecision> {
        argument_words(command)
            .into_iter()
            .find(|word| is_unresolved_argument(word))
            .map(|word| {
                PolicyDecision::new(
                    Verdict::Deny,
                    ToolClass::Privileged,
                    "unresolved_path",
                    format!("`{word}` cannot be resolved statically; shell access to it is denied"),
                )
            })
    }

    pub fn evaluate(&self, req: &ToolRequest) -> PolicyDecision {
        // 1. Every path must stay inside the authorised roots, whatever the tool.
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
            if let Err(v) = self.guard.check(Path::new(p), &base) {
                return guard_denial(p, &v);
            }
        }

        // 2. Secret files never enter model context; writing them needs a human.
        let secret = req
            .paths
            .iter()
            .find(|p| is_secret_file(Path::new(p.as_str())));
        if let Some(p) = secret {
            match req.kind {
                ToolKind::Read | ToolKind::List | ToolKind::Search => {
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
                if let Some(decision) = self.shell_argument_violation(cmd, &base) {
                    return decision;
                }
                let a = self.assess(cmd);
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

/// Representative secret names. A glob argument is denied when it could match any of them,
/// because the shell expands `.e*` to `.env` before the program runs.
const SECRET_SAMPLES: &[&str] = &[
    ".env",
    ".env.local",
    ".netrc",
    ".npmrc",
    "id_rsa",
    "id_ed25519",
    "server.pem",
    "server.key",
    "credentials",
    "secrets.json",
];

fn glob_may_match_secret(word: &str) -> bool {
    if !word.contains(['*', '?']) {
        return false;
    }
    SECRET_SAMPLES
        .iter()
        .any(|sample| wildcard_matches(word, sample))
}

/// Shell-style `*` and `?` matching over whole strings.
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
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
        // The network rule decides: the policy says Ask, and the hook turns Ask into a block (D3).
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
            assert_eq!(d.rule, "unresolved_path", "{cmd}");
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
}
