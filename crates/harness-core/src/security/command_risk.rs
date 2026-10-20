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
    /// The shell OpenCode's bash tool uses on this platform: Git Bash/WSL bash on Windows when
    /// available is still POSIX syntax, so POSIX is the default everywhere. PowerShell-specific
    /// constructs are recognised in POSIX mode too (cmdlet names, `-enc`).
    pub fn host_default() -> Self {
        Self::Posix
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

pub fn assess_command(command: &str, shell: ShellKind, extra_safe: &[String]) -> CommandAssessment {
    let _ = (command, shell, extra_safe);
    todo!("see IMPLEMENTATION SPEC")
}
