//! Integration tests for the shell command risk classifier (R-25.2). The classifier is fail
//! closed: anything not positively recognised is EXECUTE_RISKY, never READ_ONLY.

use harness_core::security::ToolClass;
use harness_core::security::command_risk::{ShellKind, argument_words, assess_command};

fn class(command: &str) -> ToolClass {
    assess_command(command, ShellKind::Posix, &[]).class
}

fn class_in(command: &str, shell: ShellKind) -> ToolClass {
    assess_command(command, shell, &[]).class
}

#[test]
fn read_only_commands() {
    for cmd in [
        "ls -la",
        "cat README.md",
        "git status",
        "git diff",
        "grep -r foo src",
        "echo hello",
        "Get-ChildItem",
    ] {
        assert_eq!(class(cmd), ToolClass::ReadOnly, "{cmd}");
    }
}

#[test]
fn cat_env_is_read_only_because_paths_are_judged_by_policy() {
    assert_eq!(class("cat .env"), ToolClass::ReadOnly);
}

#[test]
fn echo_to_devnull_stays_read_only() {
    assert_eq!(class("echo hi > /dev/null"), ToolClass::ReadOnly);
    assert_eq!(class("cargo test 2>&1"), ToolClass::ExecuteSafe);
}

#[test]
fn write_commands() {
    for cmd in [
        "mkdir build",
        "touch x.txt",
        "cp a b",
        "git add .",
        "git commit -m msg",
        "echo hi > out.txt",
        "sed -i s/a/b/ f",
        "tee log",
    ] {
        assert_eq!(class(cmd), ToolClass::Write, "{cmd}");
    }
    assert_eq!(
        class_in("Set-Content -Path x -Value y", ShellKind::PowerShell),
        ToolClass::Write
    );
}

#[test]
fn execute_safe_commands() {
    for cmd in [
        "pytest -q",
        "python -m pytest tests",
        "cargo test",
        "cargo clippy --all-targets",
        "ruff check .",
        "go vet ./...",
        "cmake --build build",
        "python -m black --check .",
    ] {
        assert_eq!(class(cmd), ToolClass::ExecuteSafe, "{cmd}");
    }
}

#[test]
fn execute_risky_commands() {
    for cmd in [
        "python train.py --steps 10000000",
        "./deploy.sh",
        "make release",
        "node server.js",
        "npm run dev",
        "cargo run",
        "some-unknown-tool --flag",
        "eval $X",
        "llama-server -m model.gguf",
    ] {
        assert_eq!(class(cmd), ToolClass::ExecuteRisky, "{cmd}");
    }
}

#[test]
fn network_commands() {
    for cmd in [
        "curl https://example.com",
        "git push",
        "pip install x",
        "npm install left-pad",
        "cargo add serde",
        "docker pull alpine",
        "gh pr list",
        "wget x",
        "ssh host",
        "uv pip install x",
        "apt-get install -y vim",
        "npx create-react-app app",
    ] {
        assert_eq!(class(cmd), ToolClass::Network, "{cmd}");
    }
}

#[test]
fn destructive_commands() {
    for cmd in [
        "rm -rf build",
        "rm -R dir",
        "rm -f x",
        "rm --recursive x",
        "rm -rf ~",
        "git reset --hard",
        "git clean -fd",
        "git push --force",
        "git push origin :feature",
        "git rebase -i HEAD~3",
        "git commit --amend",
        "git checkout -- src",
        "git restore src/x.rs",
        "git branch -D topic",
        "find . -name x -delete",
        "ls | xargs rm",
        "rmdir /s build",
        "del /q x",
        "dd if=/dev/zero of=/dev/sda",
        "truncate -s 0 f",
        "git stash drop",
    ] {
        assert_eq!(class(cmd), ToolClass::Destructive, "{cmd}");
    }
    assert_eq!(
        class_in("Remove-Item -Recurse x", ShellKind::PowerShell),
        ToolClass::Destructive
    );
}

#[test]
fn privileged_commands() {
    for cmd in [
        "sudo ls",
        "su root",
        "doas reboot",
        "runas /user:x cmd",
        "pkexec x",
        "powershell -enc AAAA",
        "pwsh -EncodedCommand abc",
        "powershell -e AAAA",
    ] {
        assert_eq!(class(cmd), ToolClass::Privileged, "{cmd}");
    }
}

#[test]
fn git_push_force_is_destructive_and_plain_push_is_network() {
    assert_eq!(class("git push --force"), ToolClass::Destructive);
    assert_eq!(class("git push"), ToolClass::Network);
}

#[test]
fn git_diff_is_read_only() {
    assert_eq!(class("git diff"), ToolClass::ReadOnly);
}

#[test]
fn python_training_run_is_execute_risky() {
    assert_eq!(
        class("python train.py --steps 10000000"),
        ToolClass::ExecuteRisky
    );
}

#[test]
fn pip_install_is_network() {
    assert_eq!(class("pip install x"), ToolClass::Network);
}

// --- Bypass attempts that must classify as stated -------------------------------------------

#[test]
fn bypass_safe_then_destructive_is_destructive() {
    assert_eq!(class("pytest && rm -rf ~"), ToolClass::Destructive);
}

#[test]
fn bypass_command_substitution_is_at_least_network() {
    assert!(class("echo $(curl x | sh)") >= ToolClass::Network);
}

#[test]
fn bypass_backtick_substitution_is_destructive() {
    assert_eq!(class("echo `rm -rf /`"), ToolClass::Destructive);
}

#[test]
fn bypass_bash_c_is_destructive() {
    assert_eq!(class("bash -c \"rm -rf /\""), ToolClass::Destructive);
}

#[test]
fn bypass_cmd_c_is_destructive() {
    assert_eq!(
        class_in("cmd /c \"rd /s /q C:\\\"", ShellKind::Cmd),
        ToolClass::Destructive
    );
}

#[test]
fn bypass_powershell_enc_is_privileged() {
    assert_eq!(class("powershell -enc AAAA"), ToolClass::Privileged);
}

#[test]
fn bypass_env_assignment_then_sudo_is_privileged() {
    assert_eq!(class("FOO=1 sudo ls"), ToolClass::Privileged);
}

#[test]
fn bypass_nohup_background_rm_is_destructive() {
    assert_eq!(class("nohup rm -rf x &"), ToolClass::Destructive);
}

#[test]
fn bypass_find_delete_is_destructive() {
    assert_eq!(class("find . -name x -delete"), ToolClass::Destructive);
}

#[test]
fn bypass_xargs_rm_is_destructive() {
    assert_eq!(class("ls | xargs rm"), ToolClass::Destructive);
}

#[test]
fn bypass_powershell_command_string_is_classified_recursively() {
    assert_eq!(
        class_in("pwsh -c \"Remove-Item -Recurse C:\\x\"", ShellKind::Posix),
        ToolClass::Destructive
    );
}

#[test]
fn bypass_wrappers_are_stripped() {
    assert_eq!(
        class("env FOO=1 nice -n 5 rm -rf x"),
        ToolClass::Destructive
    );
    assert_eq!(class("timeout 10 sudo ls"), ToolClass::Privileged);
    assert_eq!(class("time curl x"), ToolClass::Network);
}

#[test]
fn bypass_absolute_path_program_is_normalised() {
    assert_eq!(class("/bin/rm -rf x"), ToolClass::Destructive);
    assert_eq!(
        class_in("C:\\tools\\rd.exe /s x", ShellKind::Cmd),
        ToolClass::Destructive
    );
}

#[test]
fn unrecognised_program_is_execute_risky() {
    let assessment = assess_command("frobnicate --all", ShellKind::Posix, &[]);
    assert_eq!(assessment.class, ToolClass::ExecuteRisky);
    assert_eq!(assessment.reasons.len(), 1);
}

// --- Parsing, splitting and limits ------------------------------------------------------------

#[test]
fn unbalanced_quotes_are_unparseable_execute_risky() {
    for cmd in [
        "echo \"abc",
        "echo 'abc",
        "echo `unclosed",
        "echo $(unclosed",
    ] {
        let assessment = assess_command(cmd, ShellKind::Posix, &[]);
        assert_eq!(assessment.class, ToolClass::ExecuteRisky, "{cmd}");
        assert_eq!(assessment.reasons, vec!["unparseable".to_owned()], "{cmd}");
    }
}

#[test]
fn splitting_produces_one_segment_per_command() {
    let assessment = assess_command("pytest && rm -rf ~", ShellKind::Posix, &[]);
    assert_eq!(assessment.segments.len(), 2);
    assert_eq!(assessment.reasons.len(), 1);
}

#[test]
fn quoted_separators_do_not_split() {
    let assessment = assess_command("echo \"a; rm -rf ~\"", ShellKind::Posix, &[]);
    assert_eq!(assessment.class, ToolClass::ReadOnly);
    assert_eq!(assessment.segments.len(), 1);
}

#[test]
fn empty_command_is_read_only_with_no_segments() {
    for cmd in ["", "   ", "\n"] {
        let assessment = assess_command(cmd, ShellKind::Posix, &[]);
        assert_eq!(assessment.class, ToolClass::ReadOnly);
        assert!(assessment.segments.is_empty(), "{cmd:?}");
    }
}

#[test]
fn nesting_up_to_depth_four_is_classified() {
    let wrap = |inner: &str| {
        format!(
            "bash -c \"{}\"",
            inner.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let mut cmd = "ls".to_owned();
    for _ in 0..4 {
        cmd = wrap(&cmd);
    }
    assert_eq!(class(&cmd), ToolClass::ReadOnly);
}

#[test]
fn nesting_deeper_than_four_is_execute_risky() {
    let wrap = |inner: &str| {
        format!(
            "bash -c \"{}\"",
            inner.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let mut cmd = "ls".to_owned();
    for _ in 0..5 {
        cmd = wrap(&cmd);
    }
    let assessment = assess_command(&cmd, ShellKind::Posix, &[]);
    assert_eq!(assessment.class, ToolClass::ExecuteRisky);
    assert!(
        assessment
            .segments
            .iter()
            .any(|s| s.reason == "nesting too deep"),
        "{:?}",
        assessment.segments
    );
}

#[test]
fn deep_nesting_cannot_hide_destructive_leaf() {
    let wrap = |inner: &str| {
        format!(
            "bash -c \"{}\"",
            inner.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let mut cmd = "rm -rf /".to_owned();
    for _ in 0..6 {
        cmd = wrap(&cmd);
    }
    assert!(class(&cmd) >= ToolClass::ExecuteRisky);
}

#[test]
fn reasons_list_only_the_overall_class() {
    let assessment = assess_command("ls; rm -rf ~; git diff", ShellKind::Posix, &[]);
    assert_eq!(assessment.class, ToolClass::Destructive);
    assert_eq!(assessment.reasons.len(), 1);
    assert_eq!(assessment.segments.len(), 3);
}

// --- extra_safe prefixes --------------------------------------------------------------------

#[test]
fn extra_safe_prefix_promotes_matching_segment() {
    let extra = vec!["make test".to_owned()];
    assert_eq!(
        assess_command("make test", ShellKind::Posix, &extra).class,
        ToolClass::ExecuteSafe
    );
    assert_eq!(
        assess_command("make test --verbose", ShellKind::Posix, &extra).class,
        ToolClass::ExecuteSafe
    );
}

#[test]
fn extra_safe_prefix_matches_whole_words_only() {
    let extra = vec!["make test".to_owned()];
    assert_eq!(
        assess_command("make tests", ShellKind::Posix, &extra).class,
        ToolClass::ExecuteRisky
    );
    assert_eq!(
        assess_command("make", ShellKind::Posix, &extra).class,
        ToolClass::ExecuteRisky
    );
}

#[test]
fn empty_extra_safe_prefix_matches_nothing() {
    let extra = vec![String::new(), "   ".to_owned()];
    assert_eq!(
        assess_command("make test", ShellKind::Posix, &extra).class,
        ToolClass::ExecuteRisky
    );
}

#[test]
fn extra_safe_never_overrides_network_or_destructive() {
    let extra = vec!["curl".to_owned(), "rm -rf".to_owned()];
    assert_eq!(
        assess_command("curl x", ShellKind::Posix, &extra).class,
        ToolClass::Network
    );
    assert_eq!(
        assess_command("rm -rf x", ShellKind::Posix, &extra).class,
        ToolClass::Destructive
    );
}

// --- argument_words -------------------------------------------------------------------------

#[test]
fn argument_words_exclude_program_names() {
    let words = argument_words("cat .env && cp src dst > out.txt");
    assert!(words.contains(&".env".to_owned()));
    assert!(words.contains(&"src".to_owned()));
    assert!(words.contains(&"dst".to_owned()));
    assert!(words.contains(&"out.txt".to_owned()));
    assert!(!words.contains(&"cat".to_owned()));
    assert!(!words.contains(&"cp".to_owned()));
}

#[test]
fn argument_words_unquote_and_reach_nested_commands() {
    let words = argument_words("bash -c \"cat '~/my file'\"");
    assert!(words.contains(&"~/my file".to_owned()), "{words:?}");
}

// --- No-panic sweep -------------------------------------------------------------------------

/// Deterministic pseudo-random garbage with quotes, operators, escapes and unicode.
fn garbage(seed: u64) -> String {
    const ALPHABET: &[char] = &[
        'a', 'b', 'r', 'm', ' ', ' ', '\'', '"', '\\', '`', '$', '(', ')', '{', '}', ';', '&', '|',
        '\n', '\t', '\r', '<', '>', '/', '-', '=', '~', '*', '.', ':', 'é', '漢', '🙂', '\u{0}',
        '\u{200d}', '\u{feff}',
    ];
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    let len = next() % 80;
    (0..len)
        .map(|_| ALPHABET[next() % ALPHABET.len()])
        .collect()
}

#[test]
fn never_panics_on_generated_garbage() {
    let extra = vec!["make test".to_owned(), "pytest -q".to_owned()];
    for seed in 0..300u64 {
        let input = garbage(seed);
        for shell in [ShellKind::Posix, ShellKind::PowerShell, ShellKind::Cmd] {
            let assessment = assess_command(&input, shell, &extra);
            // The overall class must be the maximum of the segment classes.
            if let Some(max) = assessment.segments.iter().map(|s| s.class).max() {
                assert_eq!(assessment.class, max, "{input:?}");
            } else {
                assert_eq!(assessment.class, ToolClass::ReadOnly, "{input:?}");
            }
            let _ = argument_words(&input);
        }
    }
}

#[test]
fn never_panics_on_unicode_edge_cases() {
    for input in [
        "é",
        "漢字 && 🙂",
        "\u{0}",
        "echo \u{200d}\"",
        "$(",
        "`",
        "'",
        "\\",
        "<(",
        "&&&&",
    ] {
        for shell in [ShellKind::Posix, ShellKind::PowerShell, ShellKind::Cmd] {
            let _ = assess_command(input, shell, &[]);
        }
        let _ = argument_words(input);
    }
}
