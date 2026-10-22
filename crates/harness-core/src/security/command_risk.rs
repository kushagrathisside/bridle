//! Shell command risk classification (§25, R-25.2): the policy inspects the *actual* command text,
//! so a model cannot bypass it with creative arguments.
//!
//! IMPLEMENTATION SPEC (delegated). Pure function, no I/O, never panics on any input.
//!
//! ## Splitting
//! Tokenise respecting single quotes, double quotes (with backslash escapes in POSIX mode) and
//! PowerShell backtick escapes. Split into segments on unquoted `&&`, `||`, `;`, `|`, `&`
//! (background), and newlines. Additionally extract *inner* commands and classify them as
//! segments too: POSIX `$( ... )`, backticks `` `...` `` (POSIX only), `<( ... )`, PowerShell
//! `$( ... )` and `& { ... }` / `{ ... }` script blocks, and the string argument of
//! `bash -c`, `sh -c`, `zsh -c`, `cmd /c`, `cmd /k`, `powershell[.exe] -Command|-c`, `pwsh -c`.
//! Recursion depth limit 4; beyond it the segment is `ExecuteRisky` ("nesting too deep").
//! Unbalanced quotes → whole command `ExecuteRisky` with reason "unparseable".
//!
//! ## Per-segment classification
//! Strip leading `VAR=value` assignments and wrappers `env`, `time`, `nice`, `nohup`, `command`,
//! `exec`, `stdbuf`, `timeout <n>`, `xargs` (for xargs, classify the command it runs; `xargs rm`
//! → DESTRUCTIVE). Program name = last path component, lowercased, `.exe`/`.cmd`/`.bat`/`.ps1`
//! stripped. Then the FIRST matching rule wins:
//!
//! **PRIVILEGED**: `sudo`, `su`, `doas`, `runas`, `pkexec`, `Start-Process … -Verb RunAs`,
//! `Set-ExecutionPolicy`, `reg add|delete` on `HKLM`, `netsh`, `sc` (`config|create|delete|stop`),
//! `bcdedit`, `takeown`, `icacls`, `schtasks /create`, `systemctl`, `service`, `mount`, `umount`,
//! `chown`, `chmod` on `/` or with `-R 777`, `diskpart`, `vssadmin`, `wsl --unregister`,
//! PowerShell `-EncodedCommand`/`-enc`/`-ec`/`-e` (obfuscated payload).
//!
//! **DESTRUCTIVE**: `rm` with any of `-r -R -f --recursive --force` (incl. combined `-rf`, `-fr`)
//! or targeting `/`, `~`, `*`, `.`, `..`; `rmdir /s`, `rd /s`; `del` / `erase` with `/s` or `/q` or
//! wildcards; `Remove-Item` / `ri` / `rm` (PowerShell alias) with `-Recurse` or `-Force`;
//! `git reset --hard`, `git clean` with `-f`/`-d`/`-x`, `git checkout -- <..>` / `git checkout .`,
//! `git restore` (without `--staged`), `git push` with `--force`/`-f`/`--force-with-lease`/`--delete`/`:<ref>`,
//! `git branch -D`/`--delete --force`, `git rebase`, `git filter-branch`, `git filter-repo`,
//! `git reflog expire`, `git gc --prune`, `git stash drop|clear`, `git update-ref -d`,
//! `git commit --amend` (history rewrite, §49); `mkfs*`, `dd`, `shred`, `wipefs`, `format`,
//! `Format-Volume`, `Clear-Disk`, `cipher /w`; `find … -delete` or `find … -exec rm`;
//! `truncate`; output redirection `>` (not `>>`) onto an existing-looking path outside the
//! workspace is out of scope here — redirection is WRITE (see below).
//!
//! **NETWORK**: `curl`, `wget`, `Invoke-WebRequest`/`iwr`/`Invoke-RestMethod`/`irm`, `ssh`, `scp`,
//! `sftp`, `rsync` (with a `host:` arg), `nc`/`ncat`/`netcat`/`telnet`/`ftp`, `git clone|fetch|pull|push|ls-remote`,
//! `git submodule update`, `pip`/`pip3`/`python -m pip` `install|download`, `uv pip install`, `uv add`,
//! `uv sync`, `conda|mamba install|create`, `npm|pnpm|yarn|bun install|add|i|ci`, `npx`, `bunx`,
//! `cargo install|add|update|fetch|publish`, `apt|apt-get|dnf|yum|pacman|brew|winget|choco|scoop install`,
//! `docker pull|push|login`, `huggingface-cli download|upload`, `hf download|upload`, `gh`.
//!
//! **EXECUTE_RISKY**: arbitrary code execution: `python`/`python3`/`py` running a script or `-c`
//! (unless matched SAFE below), `node`/`deno`/`bun` running a file, `bash|sh|zsh|pwsh|powershell`
//! running a script file, `./anything`, `make`, `just`, `npm run`, `cargo run`, `docker run`,
//! `Invoke-Expression`/`iex`, `eval`, `source`/`.`; and ANY unrecognised program (reason
//! "unrecognised program").
//!
//! **EXECUTE_SAFE** (allow-list): `pytest`, `python -m pytest|unittest|mypy|ruff|black --check`,
//! `cargo test|build|check|clippy|fmt|doc|bench|tree|metadata`, `ctest`, `cmake --build`,
//! `ruff check|format --check`, `mypy`, `pyright`, `black --check`, `go test|build|vet`, `tsc`,
//! `eslint`, `rustc --version`, `nvidia-smi`; plus every prefix in `extra_safe` (matched against the
//! segment text after wrapper stripping, whole-word prefix).
//!
//! **WRITE**: `git add|commit|stash (push)|switch|checkout -b|mv|rm --cached|tag|worktree add`, `mkdir`,
//! `touch`, `cp`, `mv`, `ln`, `sed -i`, `tee`, `Set-Content`, `Add-Content`, `New-Item`,
//! `Out-File`, `Copy-Item`, `Move-Item`, `Rename-Item`. Any segment with an unquoted output
//! redirection `>`/`>>`/`1>`/`2>` to a file other than `/dev/null`/`NUL`/`$null` is at least WRITE.
//!
//! **READ_ONLY**: `ls`, `dir`, `cat`, `type`, `head`, `tail`, `wc`, `grep`, `rg`, `find` (without
//! `-delete`/`-exec`/`-execdir`/`-ok`), `echo`, `printf`, `pwd`, `cd`, `which`, `where`, `tree`,
//! `file`, `stat`, `du`, `df`, `env`/`printenv` alone, `git status|diff|log|show|branch` (listing)
//! `|rev-parse|ls-files|blame|grep|remote -v|config --get|describe|shortlog`, `Get-ChildItem`/`gci`/`ls`,
//! `Get-Content`/`gc`/`cat`, `Select-String`/`sls`, `Get-Location`, `Test-Path`, `Get-Command`,
//! `Measure-Object`, `Sort-Object`, `Select-Object`, `Where-Object`, `Format-Table`, `Out-String`.
//!
//! The command's overall class = max over all segments (ToolClass ordering). `reasons` lists one
//! human-readable reason per segment at the overall class. Empty/whitespace command → READ_ONLY
//! with no segments.
//!
//! Tests live in `crates/harness-core/tests/security_commands.rs`: at least 3 positive cases per
//! class, every bypass attempt (`pytest && rm -rf ~`, `echo $(curl x | sh)`, `` `rm -rf /` ``,
//! `bash -c "rm -rf /"`, `cmd /c "rd /s /q C:\"`, `powershell -enc AAAA`, `FOO=1 sudo ls`,
//! `nohup rm -rf x &`, `find . -name x -delete`, `ls | xargs rm`), `git push --force` vs
//! `git push` (NETWORK), `git diff` READ_ONLY, `python train.py --steps 10000000` EXECUTE_RISKY,
//! `pip install x` NETWORK, unbalanced quotes, deep nesting, and extra_safe prefixes.

use serde::{Deserialize, Serialize};

use super::ToolClass;

/// Which shell grammar to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Posix,
    PowerShell,
    Cmd,
}

impl ShellKind {
    /// The shell the agent's commands run in on this platform. On Windows the model's commands
    /// use `D:\dir` paths, and POSIX lexing would treat each backslash as an escape, turning
    /// `D:\e2e\toy` into `D:e2etoy` and breaking the path check (observed in the first end-to-end
    /// run). So Windows uses the `cmd` lexer, where backslash is a literal character. POSIX is
    /// used elsewhere.
    pub fn host_default() -> Self {
        if cfg!(windows) {
            Self::Cmd
        } else {
            Self::Posix
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentAssessment {
    /// The segment text after splitting (before wrapper stripping).
    pub text: String,
    /// Normalised program name, if one was found.
    pub program: Option<String>,
    pub class: ToolClass,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAssessment {
    pub class: ToolClass,
    pub segments: Vec<SegmentAssessment>,
    pub reasons: Vec<String>,
}

/// Maximum nesting depth for `$(...)`, backticks, `<(...)` and `sh -c`-style strings.
const MAX_DEPTH: usize = 4;

/// One lexical word: its unquoted text, plus whether any part of it was quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    text: String,
}

/// Structural pieces produced by the splitter.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Word(Word),
    /// A separator: `;`, `&&`, `||`, `|`, `&`, or a newline. Ends the current segment.
    Break,
    /// A redirection operator such as `>` or `>>` followed by its target word.
    Redirect {
        target: Option<String>,
        devnull: bool,
    },
}

/// Result of lexing a command string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lexed {
    /// Flat list of pieces, in source order.
    pieces: Vec<Piece>,
    /// Inner command texts (from `$(...)`, backticks, `<(...)`, or shell `-c` strings) that must
    /// be classified recursively.
    nested: Vec<String>,
    /// True if the input had unbalanced quotes or an unterminated `$(`/backtick.
    unparseable: bool,
}

fn is_devnull(target: &str) -> bool {
    matches!(
        target.to_ascii_lowercase().as_str(),
        "/dev/null" | "nul" | "$null"
    )
}

/// Lex one command string into pieces. Never panics: any malformed input sets `unparseable`.
///
/// POSIX quoting: `'...'` is literal, `"..."` honours `\` for `"`, `\`, `$`, `` ` `` and newline,
/// `\x` outside quotes escapes `x`. PowerShell backtick escapes are honoured in every shell kind
/// because the same classifier runs over both (the spec says PowerShell constructs are recognised
/// in POSIX mode too); a backtick outside quotes is therefore an escape, and command
/// substitution by backtick is only recognised when the shell is `Posix`.
fn lex(command: &str, shell: ShellKind) -> Lexed {
    let chars: Vec<char> = command.chars().collect();
    let mut out = Lexed {
        pieces: Vec::new(),
        nested: Vec::new(),
        unparseable: false,
    };
    let mut word = String::new();
    let mut in_word = false;
    let mut i = 0usize;

    let flush = |word: &mut String, in_word: &mut bool, pieces: &mut Vec<Piece>| {
        if *in_word {
            pieces.push(Piece::Word(Word {
                text: std::mem::take(word),
            }));
            *in_word = false;
        }
    };

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => {
                flush(&mut word, &mut in_word, &mut out.pieces);
                i += 1;
            }
            '\n' => {
                flush(&mut word, &mut in_word, &mut out.pieces);
                out.pieces.push(Piece::Break);
                i += 1;
            }
            ';' => {
                flush(&mut word, &mut in_word, &mut out.pieces);
                out.pieces.push(Piece::Break);
                i += 1;
            }
            '&' => {
                flush(&mut word, &mut in_word, &mut out.pieces);
                out.pieces.push(Piece::Break);
                i += if chars.get(i + 1) == Some(&'&') { 2 } else { 1 };
            }
            '|' => {
                flush(&mut word, &mut in_word, &mut out.pieces);
                out.pieces.push(Piece::Break);
                i += if chars.get(i + 1) == Some(&'|') { 2 } else { 1 };
            }
            '\'' => {
                in_word = true;
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    out.unparseable = true;
                    return out;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let d = chars[i];
                    if d == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if d == '\\' && shell == ShellKind::Posix {
                        match chars.get(i + 1) {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`' | '\n') => {
                                if n != '\n' {
                                    word.push(n);
                                }
                                i += 2;
                                continue;
                            }
                            _ => {
                                word.push('\\');
                                i += 1;
                                continue;
                            }
                        }
                    }
                    if d == '`' && shell == ShellKind::PowerShell {
                        // PowerShell escape inside double quotes: the next char is literal.
                        if let Some(&n) = chars.get(i + 1) {
                            word.push(n);
                            i += 2;
                            continue;
                        }
                    }
                    word.push(d);
                    i += 1;
                }
                if !closed {
                    out.unparseable = true;
                    return out;
                }
            }
            '\\' if shell == ShellKind::Posix => {
                in_word = true;
                if let Some(&n) = chars.get(i + 1) {
                    word.push(n);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '`' if shell == ShellKind::PowerShell => {
                in_word = true;
                if let Some(&n) = chars.get(i + 1) {
                    word.push(n);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '`' if shell == ShellKind::Posix => {
                // POSIX backtick command substitution.
                in_word = true;
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && chars[j] != '`' {
                    j += 1;
                }
                if j >= chars.len() {
                    out.unparseable = true;
                    return out;
                }
                let inner: String = chars[start..j].iter().collect();
                out.nested.push(inner);
                word.push_str("<subst>");
                i = j + 1;
            }
            '$' if chars.get(i + 1) == Some(&'(') && shell != ShellKind::Cmd => {
                in_word = true;
                match matching_paren(&chars, i + 1) {
                    Some(end) => {
                        let inner: String = chars[i + 2..end].iter().collect();
                        out.nested.push(inner);
                        word.push_str("<subst>");
                        i = end + 1;
                    }
                    None => {
                        out.unparseable = true;
                        return out;
                    }
                }
            }
            '<' | '>' => {
                // `<(...)` process substitution (POSIX) or a redirection.
                if c == '<' && chars.get(i + 1) == Some(&'(') && shell == ShellKind::Posix {
                    in_word = true;
                    match matching_paren(&chars, i + 1) {
                        Some(end) => {
                            let inner: String = chars[i + 2..end].iter().collect();
                            out.nested.push(inner);
                            word.push_str("<subst>");
                            i = end + 1;
                        }
                        None => {
                            out.unparseable = true;
                            return out;
                        }
                    }
                    continue;
                }
                if in_word && !word.is_empty() && word.chars().all(|d| d.is_ascii_digit()) {
                    // A bare file-descriptor number such as the `2` in `2>file`.
                    word.clear();
                    in_word = false;
                }
                flush(&mut word, &mut in_word, &mut out.pieces);
                let mut j = i + 1;
                if c == '>' && chars.get(j) == Some(&'>') {
                    j += 1;
                }
                let target = read_redirect_target(&chars, &mut j);
                // Input redirections and fd duplications (`>&2`) never write a file.
                let devnull = c == '<'
                    || target
                        .as_deref()
                        .is_some_and(|t| t.starts_with('&') || is_devnull(t));
                out.pieces.push(Piece::Redirect { target, devnull });
                i = j;
            }
            _ => {
                in_word = true;
                word.push(c);
                i += 1;
            }
        }
    }
    flush(&mut word, &mut in_word, &mut out.pieces);
    out
}

/// Given `chars[open] == '('`, return the index of the matching `)`, honouring nested parens and
/// simple quotes. `None` if unbalanced.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    i += 1;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => i += 1,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Read the target word after a redirection operator, skipping blanks and a leading `&`.
fn read_redirect_target(chars: &[char], j: &mut usize) -> Option<String> {
    while chars.get(*j).is_some_and(|c| *c == ' ' || *c == '\t') {
        *j += 1;
    }
    let mut target = String::new();
    if chars.get(*j) == Some(&'&') {
        // fd duplication such as `>&2` or `2>&1`: keep a leading `&` marker.
        target.push('&');
        *j += 1;
    }
    while let Some(&c) = chars.get(*j) {
        if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>') {
            break;
        }
        if c == '\'' || c == '"' {
            *j += 1;
            while let Some(&d) = chars.get(*j) {
                *j += 1;
                if d == c {
                    break;
                }
                target.push(d);
            }
            continue;
        }
        target.push(c);
        *j += 1;
    }
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

fn leaf(class: ToolClass, reason: &str) -> Verdict {
    Verdict::Leaf(class, reason.to_owned())
}

/// Letters of a POSIX-style short flag cluster such as `-rf` (returns `rf`). `None` for anything
/// that is not one (`--long`, `-`, `/s`, a path, a number).
fn short_letters(arg: &str) -> Option<&str> {
    let rest = arg.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with('-') || !rest.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(rest)
}

/// Whether `arg` is a short flag cluster that contains the letter `c` (case-sensitive).
fn has_short(arg: &str, c: char) -> bool {
    short_letters(arg).is_some_and(|l| l.contains(c))
}

/// Whether the configured prefix (whole words, first word is the program) matches this segment.
fn prefix_matches(prefix: &str, program: &str, args: &[String]) -> bool {
    let words: Vec<&str> = prefix.split_whitespace().collect();
    let Some((first, rest)) = words.split_first() else {
        // An empty prefix must never match everything.
        return false;
    };
    if normalise_program(first) != program {
        return false;
    }
    rest.iter()
        .enumerate()
        .all(|(i, w)| args.get(i).is_some_and(|a| a == w))
}

/// Classify a `git` invocation by its subcommand. Always returns a verdict.
fn classify_git(args: &[String]) -> Verdict {
    // Skip global options such as `-C <path>` and `-c key=value`.
    let mut i = 0usize;
    while let Some(a) = args.get(i) {
        if matches!(
            a.as_str(),
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    let Some(sub) = args.get(i).map(|s| s.to_ascii_lowercase()) else {
        return leaf(ToolClass::ExecuteRisky, "git without a subcommand");
    };
    let rest: &[String] = args.get(i + 1..).unwrap_or(&[]);
    let lower: Vec<String> = rest.iter().map(|a| a.to_ascii_lowercase()).collect();
    let has = |f: &str| lower.iter().any(|a| a == f);
    let first_pos = rest
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|a| a.to_ascii_lowercase())
        .unwrap_or_default();

    match sub.as_str() {
        "reset" if has("--hard") => leaf(ToolClass::Destructive, "git reset --hard discards work"),
        "clean"
            if has("--force")
                || lower
                    .iter()
                    .any(|a| short_letters(a).is_some_and(|l| l.contains(['f', 'd', 'x']))) =>
        {
            leaf(ToolClass::Destructive, "git clean removes untracked files")
        }
        "checkout" if rest.iter().any(|a| a == "--" || a == ".") => leaf(
            ToolClass::Destructive,
            "git checkout -- discards working-tree changes",
        ),
        "checkout" if has("-b") || has("-B") => {
            leaf(ToolClass::Write, "git checkout -b creates a branch")
        }
        "restore" if !has("--staged") => leaf(
            ToolClass::Destructive,
            "git restore discards working-tree changes",
        ),
        "restore" => leaf(
            ToolClass::Write,
            "git restore --staged only changes the index",
        ),
        "push" => {
            let force_or_delete = lower.iter().any(|a| {
                matches!(a.as_str(), "--force" | "--delete" | "--force-if-includes")
                    || a.starts_with("--force-with-lease")
                    || a.starts_with(':')
                    || has_short(a, 'f')
                    || has_short(a, 'd')
            });
            if force_or_delete {
                leaf(
                    ToolClass::Destructive,
                    "git push forces or deletes remote refs",
                )
            } else {
                leaf(ToolClass::Network, "git push contacts a remote")
            }
        }
        "branch" if rest.iter().any(|a| a == "-D") || (has("--delete") && has("--force")) => {
            leaf(ToolClass::Destructive, "git branch -D deletes a branch")
        }
        "branch"
            if lower.iter().all(|a| {
                matches!(
                    a.as_str(),
                    "-a" | "-r"
                        | "-v"
                        | "-vv"
                        | "-l"
                        | "--list"
                        | "--all"
                        | "--remotes"
                        | "--verbose"
                        | "--show-current"
                        | "--no-color"
                        | "--color"
                )
            }) =>
        {
            leaf(ToolClass::ReadOnly, "git branch lists branches")
        }
        "rebase" => leaf(ToolClass::Destructive, "git rebase rewrites history"),
        "filter-branch" | "filter-repo" => leaf(
            ToolClass::Destructive,
            "git history filter rewrites history",
        ),
        "reflog" if first_pos == "expire" => leaf(ToolClass::Destructive, "git reflog expire"),
        "gc" if lower.iter().any(|a| a.starts_with("--prune")) => {
            leaf(ToolClass::Destructive, "git gc --prune deletes objects")
        }
        "stash" if matches!(first_pos.as_str(), "drop" | "clear") => leaf(
            ToolClass::Destructive,
            "git stash drop or clear discards stashes",
        ),
        "stash" if matches!(first_pos.as_str(), "list" | "show") => {
            leaf(ToolClass::ReadOnly, "git stash lists stashes")
        }
        "stash" if first_pos.is_empty() || matches!(first_pos.as_str(), "push" | "save") => {
            leaf(ToolClass::Write, "git stash stores working-tree changes")
        }
        "update-ref" if has("-d") => {
            leaf(ToolClass::Destructive, "git update-ref -d deletes a ref")
        }
        "commit" if has("--amend") => leaf(
            ToolClass::Destructive,
            "git commit --amend rewrites history",
        ),
        "add" | "commit" | "switch" | "mv" | "tag" => leaf(
            ToolClass::Write,
            &format!("git {sub} changes the repository"),
        ),
        "clone" | "fetch" | "pull" | "ls-remote" | "submodule" => {
            leaf(ToolClass::Network, &format!("git {sub} contacts a remote"))
        }
        "status" | "diff" | "log" | "show" | "rev-parse" | "ls-files" | "blame" | "grep"
        | "describe" | "shortlog" => leaf(ToolClass::ReadOnly, &format!("git {sub} only reads")),
        "config" if has("--get") || has("--get-all") || has("--list") => {
            leaf(ToolClass::ReadOnly, "git config reads a value")
        }
        "remote" if lower.iter().all(|a| a == "-v" || a == "--verbose") => {
            leaf(ToolClass::ReadOnly, "git remote lists remotes")
        }
        _ => leaf(
            ToolClass::ExecuteRisky,
            &format!("unrecognised git subcommand {sub}"),
        ),
    }
}

/// `bash -c STRING` style: the first flag containing `c` before any positional argument; the
/// string is the argument after it.
fn shell_c_argument(args: &[String]) -> Option<String> {
    for (idx, a) in args.iter().enumerate() {
        if !a.starts_with('-') {
            return None;
        }
        if has_short(a, 'c') {
            return args.get(idx + 1).cloned();
        }
    }
    None
}

/// Classify one program with its arguments, then apply the configured safe prefixes.
///
/// A configured prefix only upgrades a result that would otherwise be EXECUTE_RISKY (for example
/// `make test`). It can never lower a WRITE, NETWORK, DESTRUCTIVE or PRIVILEGED result, and it does
/// not relabel READ_ONLY commands, so the logged class stays accurate.
fn classify_program(
    program: &str,
    args: &[String],
    in_xargs: bool,
    extra_safe: &[String],
) -> Verdict {
    match classify_builtin(program, args, in_xargs) {
        Verdict::Leaf(ToolClass::ExecuteRisky, _)
            if extra_safe.iter().any(|p| prefix_matches(p, program, args)) =>
        {
            leaf(ToolClass::ExecuteSafe, "matches a configured safe prefix")
        }
        other => other,
    }
}

/// Built-in rules, tried in the spec's order: PRIVILEGED, DESTRUCTIVE, NETWORK, SAFE, RISKY,
/// WRITE, READ_ONLY, and finally the fail-closed default.
fn classify_builtin(program: &str, args: &[String], in_xargs: bool) -> Verdict {
    use ToolClass::{Destructive, ExecuteRisky, ExecuteSafe, Network, Privileged, ReadOnly, Write};

    let lower: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
    let pos: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|a| a.to_ascii_lowercase())
        .collect();
    let pos0: &str = pos.first().map(String::as_str).unwrap_or("");
    let pos1: &str = pos.get(1).map(String::as_str).unwrap_or("");

    // PRIVILEGED
    if matches!(program, "sudo" | "su" | "doas" | "runas" | "pkexec") {
        return leaf(Privileged, &format!("{program} escalates privileges"));
    }
    if matches!(program, "powershell" | "pwsh")
        && lower
            .iter()
            .any(|a| matches!(a.as_str(), "-encodedcommand" | "-enc" | "-ec" | "-e"))
    {
        return leaf(Privileged, "encoded PowerShell payload hides its command");
    }
    if program == "start-process"
        && lower.iter().any(|a| a == "-verb")
        && lower.iter().any(|a| a == "runas")
    {
        return leaf(Privileged, "Start-Process -Verb RunAs elevates privileges");
    }

    // DESTRUCTIVE
    if in_xargs && program == "rm" {
        return leaf(Destructive, "xargs feeds arguments to rm");
    }
    if matches!(program, "rm" | "remove-item" | "ri") {
        let recursive_or_force = if program == "rm" {
            args.iter().any(|a| {
                a == "--recursive"
                    || a == "--force"
                    || has_short(a, 'r')
                    || has_short(a, 'R')
                    || has_short(a, 'f')
            })
        } else {
            lower.iter().any(|a| a == "-recurse" || a == "-force")
        };
        if recursive_or_force {
            return leaf(Destructive, "recursive or forced removal");
        }
        let root_target = pos
            .iter()
            .any(|a| matches!(a.as_str(), "/" | "~" | "*" | "." | ".." | "/*" | "~/"));
        if root_target {
            return leaf(Destructive, "removal targets /, ~ or a wildcard");
        }
    }
    if matches!(program, "rmdir" | "rd") && lower.iter().any(|a| a == "/s") {
        return leaf(Destructive, "rmdir /s removes a whole tree");
    }
    if matches!(program, "del" | "erase")
        && lower
            .iter()
            .any(|a| a == "/s" || a == "/q" || a.contains('*'))
    {
        return leaf(Destructive, "del with /s, /q or a wildcard");
    }
    if program == "git" {
        return classify_git(args);
    }
    if program.starts_with("mkfs")
        || matches!(
            program,
            "dd" | "shred"
                | "wipefs"
                | "format"
                | "format-volume"
                | "clear-disk"
                | "diskpart"
                | "truncate"
        )
    {
        return leaf(
            Destructive,
            &format!("{program} overwrites or destroys data"),
        );
    }
    if program == "cipher" && lower.iter().any(|a| a == "/w") {
        return leaf(Destructive, "cipher /w wipes free space");
    }
    if program == "find" {
        if lower.iter().any(|a| {
            matches!(
                a.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            )
        }) {
            return leaf(Destructive, "find with -delete or -exec");
        }
        return leaf(ReadOnly, "find lists files");
    }

    // NETWORK
    if matches!(
        program,
        "curl"
            | "wget"
            | "invoke-webrequest"
            | "iwr"
            | "invoke-restmethod"
            | "irm"
            | "ssh"
            | "scp"
            | "sftp"
            | "nc"
            | "ncat"
            | "netcat"
            | "telnet"
            | "ftp"
            | "gh"
            | "huggingface-cli"
            | "hf"
    ) {
        return leaf(Network, &format!("{program} contacts the network"));
    }
    if program == "rsync" && args.iter().any(|a| a.contains(':')) {
        return leaf(Network, "rsync to a remote host");
    }
    if matches!(program, "python" | "python3" | "py")
        && lower.first().map(String::as_str) == Some("-m")
        && lower.get(1).map(String::as_str) == Some("pip")
        && lower.iter().any(|a| a == "install" || a == "download")
    {
        return leaf(Network, "pip install or download contacts the network");
    }
    if matches!(program, "pip" | "pip3") && matches!(pos0, "install" | "download") {
        return leaf(Network, "pip install or download contacts the network");
    }
    if program == "uv" && ((pos0 == "pip" && pos1 == "install") || matches!(pos0, "add" | "sync")) {
        return leaf(Network, "uv package operation contacts the network");
    }
    if matches!(program, "conda" | "mamba") && matches!(pos0, "install" | "create") {
        return leaf(Network, "conda or mamba install contacts the network");
    }
    if matches!(program, "npm" | "pnpm" | "yarn" | "bun")
        && matches!(pos0, "install" | "add" | "i" | "ci")
    {
        return leaf(Network, "package install contacts the network");
    }
    if matches!(program, "npx" | "bunx") {
        return leaf(Network, "npx downloads and runs a package");
    }
    if program == "cargo" && matches!(pos0, "install" | "add" | "update" | "fetch" | "publish") {
        return leaf(Network, "cargo contacts the registry");
    }
    if matches!(
        program,
        "apt" | "apt-get" | "dnf" | "yum" | "pacman" | "brew" | "winget" | "choco" | "scoop"
    ) && pos.iter().any(|a| a == "install")
    {
        return leaf(Network, "system package install contacts the network");
    }
    if program == "docker" && matches!(pos0, "pull" | "push" | "login") {
        return leaf(Network, "docker contacts a registry");
    }

    // EXECUTE_SAFE: the built-in allow-list (configured prefixes are applied by the caller).
    if program == "pytest" {
        return leaf(ExecuteSafe, "pytest runs tests");
    }
    if matches!(program, "python" | "python3" | "py")
        && lower.first().map(String::as_str) == Some("-m")
    {
        let module = lower.get(1).map(String::as_str).unwrap_or("");
        let safe_module = match module {
            "pytest" | "unittest" | "mypy" | "ruff" => true,
            "black" => lower.iter().any(|a| a == "--check"),
            _ => false,
        };
        if safe_module {
            return leaf(ExecuteSafe, "python test or lint module");
        }
    }
    if program == "cargo"
        && matches!(
            pos0,
            "test" | "build" | "check" | "clippy" | "fmt" | "doc" | "tree" | "metadata"
        )
    {
        return leaf(ExecuteSafe, "cargo test or build tooling");
    }
    if program == "go" && matches!(pos0, "test" | "build" | "vet") {
        return leaf(ExecuteSafe, "go test or build tooling");
    }
    if program == "cmake" && lower.first().map(String::as_str) == Some("--build") {
        return leaf(ExecuteSafe, "cmake --build");
    }
    if program == "ruff"
        && (pos0 == "check" || (pos0 == "format" && lower.iter().any(|a| a == "--check")))
    {
        return leaf(ExecuteSafe, "ruff check or format check");
    }
    if matches!(
        program,
        "ctest" | "mypy" | "pyright" | "tsc" | "eslint" | "nvidia-smi"
    ) {
        return leaf(ExecuteSafe, "allow-listed checker or tool");
    }

    // EXECUTE_RISKY: code execution, including nested shell strings.
    if matches!(program, "python" | "python3" | "py") {
        return leaf(ExecuteRisky, "runs a Python program");
    }
    if matches!(program, "node" | "deno") {
        return leaf(ExecuteRisky, "runs JavaScript code");
    }
    if program == "bun" {
        return leaf(ExecuteRisky, "bun runs code");
    }
    if matches!(program, "bash" | "sh" | "zsh") {
        return match shell_c_argument(args) {
            Some(inner) => Verdict::Nested(inner, ShellKind::Posix),
            None => leaf(ExecuteRisky, "runs a shell script"),
        };
    }
    if matches!(program, "pwsh" | "powershell") {
        if let Some(idx) = lower.iter().position(|a| a == "-c" || a == "-command") {
            let inner = args.get(idx + 1..).unwrap_or(&[]).join(" ");
            if !inner.is_empty() {
                return Verdict::Nested(inner, ShellKind::PowerShell);
            }
        }
        return leaf(ExecuteRisky, "runs a PowerShell script");
    }
    if program == "cmd" {
        if let Some(idx) = lower.iter().position(|a| a == "/c" || a == "/k") {
            let inner = args.get(idx + 1..).unwrap_or(&[]).join(" ");
            if !inner.is_empty() {
                return Verdict::Nested(inner, ShellKind::Cmd);
            }
        }
        return leaf(ExecuteRisky, "runs a command shell");
    }
    if matches!(
        program,
        "make" | "just" | "llama-server" | "invoke-expression" | "iex" | "eval" | "source" | "."
    ) {
        return leaf(ExecuteRisky, &format!("runs code via {program}"));
    }
    if program == "npm" && matches!(pos0, "run" | "run-script" | "exec") {
        return leaf(ExecuteRisky, "npm run executes a project script");
    }
    if program == "cargo" && pos0 == "run" {
        return leaf(ExecuteRisky, "cargo run executes code");
    }
    if program == "docker" && pos0 == "run" {
        return leaf(ExecuteRisky, "docker run starts a container");
    }

    // WRITE
    if matches!(
        program,
        "mkdir"
            | "touch"
            | "cp"
            | "mv"
            | "ln"
            | "tee"
            | "set-content"
            | "add-content"
            | "new-item"
            | "out-file"
            | "copy-item"
            | "move-item"
            | "rename-item"
    ) {
        return leaf(Write, &format!("{program} writes files"));
    }
    if program == "sed" && args.iter().any(|a| a == "--in-place" || has_short(a, 'i')) {
        return leaf(Write, "sed -i edits files in place");
    }

    // READ_ONLY
    if matches!(
        program,
        "ls" | "dir"
            | "cat"
            | "type"
            | "head"
            | "tail"
            | "wc"
            | "grep"
            | "rg"
            | "echo"
            | "printf"
            | "pwd"
            | "cd"
            | "which"
            | "where"
            | "file"
            | "stat"
            | "du"
            | "df"
            | "printenv"
            | "get-childitem"
            | "gci"
            | "get-content"
            | "gc"
            | "select-string"
            | "sls"
            | "get-location"
            | "test-path"
            | "get-command"
            | "measure-object"
            | "sort-object"
            | "select-object"
            | "where-object"
            | "format-table"
            | "out-string"
    ) {
        return leaf(ReadOnly, &format!("{program} only reads"));
    }
    if program == "env" && args.is_empty() {
        return leaf(ReadOnly, "env prints the environment");
    }

    // Fail closed.
    leaf(ExecuteRisky, &format!("unrecognised program {program}"))
}

/// One analysed segment plus the argument words it carries (used by [`argument_words`]).
#[derive(Debug, Clone)]
struct Analysed {
    segment: SegmentAssessment,
    args: Vec<String>,
}

/// What a classification rule decided for a segment's program.
enum Verdict {
    Leaf(ToolClass, String),
    /// The program runs an inner command string (`bash -c`, `cmd /c`, `powershell -c`).
    Nested(String, ShellKind),
}

/// Split and classify `command` at nesting `depth`, recursing into inner commands.
fn analyse(command: &str, shell: ShellKind, extra_safe: &[String], depth: usize) -> Vec<Analysed> {
    if depth > MAX_DEPTH {
        return vec![Analysed {
            segment: SegmentAssessment {
                text: command.to_owned(),
                program: None,
                class: ToolClass::ExecuteRisky,
                reason: "nesting too deep".to_owned(),
            },
            args: Vec::new(),
        }];
    }
    let lexed = lex(command, shell);
    if lexed.unparseable {
        return vec![Analysed {
            segment: SegmentAssessment {
                text: command.to_owned(),
                program: None,
                class: ToolClass::ExecuteRisky,
                reason: "unparseable".to_owned(),
            },
            args: Vec::new(),
        }];
    }

    let mut out = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    let mut groups: Vec<Vec<Piece>> = Vec::new();
    for piece in lexed.pieces {
        if piece == Piece::Break {
            groups.push(std::mem::take(&mut current));
        } else {
            current.push(piece);
        }
    }
    groups.push(current);

    for group in groups {
        out.extend(analyse_segment(&group, extra_safe, depth));
    }
    for inner in &lexed.nested {
        out.extend(analyse(inner, shell, extra_safe, depth + 1));
    }
    out
}

/// Classify one segment (a run of words and redirections between separators).
fn analyse_segment(pieces: &[Piece], extra_safe: &[String], depth: usize) -> Vec<Analysed> {
    let mut words: Vec<String> = Vec::new();
    let mut write_redirect = false;
    let mut redirect_targets: Vec<String> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Word(w) => words.push(w.text.clone()),
            Piece::Redirect { target, devnull } => {
                if let Some(t) = target {
                    redirect_targets.push(t.clone());
                }
                if !devnull {
                    write_redirect = true;
                }
            }
            Piece::Break => {}
        }
    }
    if words.is_empty() && !write_redirect {
        return Vec::new();
    }

    let text = words.join(" ");
    let Some(stripped) = strip_wrappers(words) else {
        // Only assignments or nothing runnable: not positively recognised.
        return vec![Analysed {
            segment: SegmentAssessment {
                text,
                program: None,
                class: if write_redirect {
                    ToolClass::Write
                } else {
                    ToolClass::ExecuteRisky
                },
                reason: if write_redirect {
                    "output redirection".to_owned()
                } else {
                    "no runnable program".to_owned()
                },
            },
            args: redirect_targets,
        }];
    };

    let program = normalise_program(&stripped.words[0]);
    let raw_program = stripped.words[0].as_str();
    let args: Vec<String> = stripped.words[1..].to_vec();
    let mut arg_words = args.clone();
    arg_words.extend(redirect_targets.iter().cloned());

    let verdict = if raw_program.starts_with("./")
        || raw_program.starts_with("../")
        || raw_program.starts_with(".\\")
        || raw_program.starts_with("..\\")
    {
        Verdict::Leaf(
            ToolClass::ExecuteRisky,
            format!("runs local script {raw_program}"),
        )
    } else {
        classify_program(&program, &args, stripped.in_xargs, extra_safe)
    };

    match verdict {
        Verdict::Nested(inner, inner_shell) => {
            let mut out = Vec::new();
            if write_redirect {
                out.push(Analysed {
                    segment: SegmentAssessment {
                        text: text.clone(),
                        program: Some(program.clone()),
                        class: ToolClass::Write,
                        reason: "output redirection".to_owned(),
                    },
                    args: redirect_targets.clone(),
                });
            }
            out.extend(analyse(&inner, inner_shell, extra_safe, depth + 1));
            out
        }
        Verdict::Leaf(mut class, mut reason) => {
            if write_redirect && class < ToolClass::Write {
                class = ToolClass::Write;
                reason = "output redirection to a file".to_owned();
            }
            vec![Analysed {
                segment: SegmentAssessment {
                    text,
                    program: Some(program),
                    class,
                    reason,
                },
                args: arg_words,
            }]
        }
    }
}

/// The words left after removing assignments and wrapper commands.
struct Stripped {
    words: Vec<String>,
    /// True when the command runs under `xargs`, so the classified program receives many args.
    in_xargs: bool,
}

/// Lower-cased basename of a program word, without a Windows executable suffix.
fn normalise_program(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut lower = base.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = lower.strip_suffix(suffix) {
            lower = stem.to_owned();
            break;
        }
    }
    lower
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Remove leading assignments and wrapper commands. Returns `None` when no program remains.
/// A bare `env` or `printenv` with nothing after it is kept as the program, since it only prints.
fn strip_wrappers(mut words: Vec<String>) -> Option<Stripped> {
    let mut in_xargs = false;
    let mut last_wrapper: Option<String> = None;
    while let Some(first) = words.first().cloned() {
        if is_assignment(&first) {
            words.remove(0);
            continue;
        }
        let name = normalise_program(&first);
        let skip_value_opts: &[&str] = match name.as_str() {
            "env" => &["-u", "-C", "-S"],
            "time" | "nohup" | "command" | "exec" => &["-a"],
            "nice" => &["-n"],
            "stdbuf" => &["-o", "-e", "-i"],
            "timeout" => &["-s", "-k"],
            "xargs" => &["-n", "-I", "-P", "-d", "-L", "-s", "-E", "-a"],
            _ => break,
        };
        words.remove(0);
        last_wrapper = Some(name.clone());
        if name == "xargs" {
            in_xargs = true;
        }
        // Skip the wrapper's own options, including those that take a value.
        while let Some(opt) = words.first().cloned() {
            if skip_value_opts.contains(&opt.as_str()) {
                words.remove(0);
                if !words.is_empty() {
                    words.remove(0);
                }
            } else if opt.starts_with('-') || is_assignment(&opt) {
                words.remove(0);
            } else {
                break;
            }
        }
        if name == "timeout" && !words.is_empty() {
            // The duration operand, e.g. `timeout 10 cmd`.
            words.remove(0);
        }
    }
    if words.is_empty() {
        if last_wrapper.as_deref() == Some("env") {
            return Some(Stripped {
                words: vec!["env".to_owned()],
                in_xargs: false,
            });
        }
        return None;
    }
    Some(Stripped { words, in_xargs })
}

pub fn assess_command(command: &str, shell: ShellKind, extra_safe: &[String]) -> CommandAssessment {
    let segments: Vec<SegmentAssessment> = analyse(command, shell, extra_safe, 0)
        .into_iter()
        .map(|a| a.segment)
        .collect();
    let class = segments
        .iter()
        .map(|s| s.class)
        .max()
        .unwrap_or(ToolClass::ReadOnly);
    let reasons = segments
        .iter()
        .filter(|s| s.class == class)
        .map(|s| s.reason.clone())
        .collect();
    CommandAssessment {
        class,
        segments,
        reasons,
    }
}

/// The unquoted argument words of every segment (including nested commands and redirection
/// targets), excluding program names. Used by the policy layer to check file paths.
pub fn argument_words(command: &str) -> Vec<String> {
    analyse(command, ShellKind::host_default(), &[], 0)
        .into_iter()
        .flat_map(|a| a.args)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn program_names_are_normalised() {
        assert_eq!(normalise_program("/usr/bin/LS"), "ls");
        assert_eq!(normalise_program("C:\\Tools\\Rd.EXE"), "rd");
        assert_eq!(normalise_program("build.ps1"), "build");
    }

    #[test]
    fn short_flag_clusters_are_recognised() {
        assert_eq!(short_letters("-rf"), Some("rf"));
        assert_eq!(short_letters("--force"), None);
        assert_eq!(short_letters("/s"), None);
        assert!(has_short("-fr", 'r'));
        assert!(!has_short("--recursive", 'r'));
    }

    #[test]
    fn shell_c_string_is_the_following_argument() {
        let args = vec!["-lc".to_owned(), "ls -la".to_owned()];
        assert_eq!(shell_c_argument(&args), Some("ls -la".to_owned()));
        let script = vec!["script.sh".to_owned(), "-c".to_owned()];
        assert_eq!(shell_c_argument(&script), None);
    }

    #[test]
    fn assignments_are_recognised_only_with_identifier_names() {
        assert!(is_assignment("FOO=1"));
        assert!(is_assignment("_x="));
        assert!(!is_assignment("=1"));
        assert!(!is_assignment("--opt=1"));
        assert!(!is_assignment("plain"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod windows_shell_tests {
    use super::*;

    /// Regression: POSIX lexing turned `D:\e2e\toy` into `D:e2etoy` (first end-to-end run).
    #[test]
    fn cmd_lexing_keeps_windows_paths_intact() {
        let words: Vec<String> =
            analyse(r"cd D:\e2e\toy && python -m pytest", ShellKind::Cmd, &[], 0)
                .into_iter()
                .flat_map(|a| a.args)
                .collect();
        assert!(words.contains(&r"D:\e2e\toy".to_owned()), "{words:?}");
    }

    #[test]
    fn posix_lexing_still_treats_backslash_as_escape() {
        let words: Vec<String> = analyse(r"cat a\ b", ShellKind::Posix, &[], 0)
            .into_iter()
            .flat_map(|a| a.args)
            .collect();
        assert_eq!(words, vec!["a b".to_owned()]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod extra_safe_tests {
    use super::*;

    fn class(cmd: &str, extra: &[&str]) -> ToolClass {
        let extra: Vec<String> = extra.iter().map(|s| (*s).to_owned()).collect();
        assess_command(cmd, ShellKind::Posix, &extra).class
    }

    #[test]
    fn a_safe_prefix_upgrades_only_risky_commands() {
        assert_eq!(class("make test", &["make test"]), ToolClass::ExecuteSafe);
        assert_eq!(class("make test", &[]), ToolClass::ExecuteRisky);
    }

    #[test]
    fn a_safe_prefix_never_lowers_or_relabels_other_classes() {
        assert_eq!(class("cp a b", &["cp"]), ToolClass::Write);
        assert_eq!(class("rm -rf build", &["rm"]), ToolClass::Destructive);
        assert_eq!(class("curl https://x", &["curl"]), ToolClass::Network);
        assert_eq!(class("sudo ls", &["sudo"]), ToolClass::Privileged);
        assert_eq!(class("ls src", &["ls"]), ToolClass::ReadOnly);
    }
}
