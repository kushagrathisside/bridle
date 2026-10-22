//! `git_history` and `git_log_range`: commit history from git, bounded and redacted.
//!
//! Both read the log with a machine format (unit and record separators), never a shell. Paths
//! pass the read policy, revisions are checked against a strict character set and must resolve to
//! a commit, and `--end-of-options` keeps any revision from being read as an option. Author
//! emails are not reported.

use serde::Serialize;
use serde_json::{Value, json};

use super::{
    DiffStat, LIST_LIMIT, McpTool, ToolContext, ToolOutput, bool_arg, check_read_path, lossy,
    parse_shortstat, redacted_output, run_git, u64_arg,
};

const DEFAULT_HISTORY_COUNT: u64 = 20;
const MAX_HISTORY_COUNT: u64 = 100;
const DEFAULT_RANGE_COUNT: u64 = 50;
const MAX_RANGE_COUNT: u64 = 200;
/// Files listed per commit; the full count is reported beside them.
const FILES_PER_COMMIT: usize = 10;
/// The most log output read; the commit count bounds it long before this in practice.
const LOG_LIMIT_BYTES: usize = 1024 * 1024;
/// The longest revision accepted (branch names, tags, hashes, `HEAD~3`).
const MAX_REV_LEN: usize = 200;
/// One commit: record separator, then hash, short hash, author, ISO date and subject separated by
/// unit separators. `--name-only` lists files after it.
const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%an%x1f%aI%x1f%s";

/// One commit from `git log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// Changed files (at most [`FILES_PER_COMMIT`]), when requested.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The full number of changed files, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_count: Option<usize>,
}

/// Parses `git log` output written with [`LOG_FORMAT`], with or without `--name-only`.
pub fn parse_log(text: &str, with_files: bool) -> Vec<Commit> {
    text.split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let mut lines = record.lines();
            let header = lines.next()?;
            let mut fields = header.split('\u{1f}');
            let mut commit = Commit {
                hash: fields.next()?.to_owned(),
                short: fields.next()?.to_owned(),
                author: fields.next()?.to_owned(),
                date: fields.next()?.to_owned(),
                subject: fields.next().unwrap_or_default().to_owned(),
                files: Vec::new(),
                files_count: None,
            };
            if with_files {
                let files: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();
                commit.files_count = Some(files.len());
                commit.files = files
                    .into_iter()
                    .take(FILES_PER_COMMIT)
                    .map(str::to_owned)
                    .collect();
            }
            Some(commit)
        })
        .collect()
}

/// Whether `rev` looks like a revision and nothing else: no option, range, path or whitespace.
pub fn is_plain_revision(rev: &str) -> bool {
    !rev.is_empty()
        && rev.len() <= MAX_REV_LEN
        && !rev.starts_with('-')
        && !rev.contains("..")
        && rev
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/~^@{}-".contains(c))
}

/// Runs `git log` with the shared format and returns at most `max` commits, plus whether more exist.
fn read_log(
    ctx: &ToolContext,
    options: &[&str],
    tail: &[&str],
    max: u64,
    with_files: bool,
) -> Result<(Vec<Commit>, bool), String> {
    // One extra commit tells us whether the list was cut.
    let count = (max + 1).to_string();
    let mut args = vec!["log", "--no-color", LOG_FORMAT, "-n", &count];
    if with_files {
        args.extend(["--name-only", "--relative"]);
    }
    args.extend_from_slice(options);
    args.extend_from_slice(tail);
    let raw = lossy(&run_git(ctx, &args, LOG_LIMIT_BYTES)?);
    let mut commits = parse_log(&raw, with_files);
    let more = commits.len() as u64 > max;
    commits.truncate(usize::try_from(max).unwrap_or(usize::MAX));
    Ok((commits, more))
}

fn render(commits: &[Commit], more: bool) -> String {
    let mut out = String::new();
    for c in commits {
        let day = c.date.get(..10).unwrap_or(&c.date);
        out.push_str(&format!("{} {day} {} ({})\n", c.short, c.subject, c.author));
        for file in &c.files {
            out.push_str(&format!("    {file}\n"));
        }
        if let Some(count) = c.files_count
            && count > c.files.len()
        {
            out.push_str(&format!("    ... and {} more\n", count - c.files.len()));
        }
    }
    if commits.is_empty() {
        out.push_str("No commits.\n");
    }
    if more {
        out.push_str("[... more commits; raise max_count or narrow the range ...]\n");
    }
    out
}

fn count_arg(args: &Value, default: u64, max: u64) -> Result<u64, String> {
    Ok(u64_arg(args, "max_count", default)?.clamp(1, max))
}

/// `git_history`: the commits that touched a path (the whole workspace by default).
#[derive(Debug)]
pub struct GitHistoryTool;

impl McpTool for GitHistoryTool {
    fn name(&self) -> &'static str {
        "git_history"
    }
    fn description(&self) -> &'static str {
        "List the commits that changed a file or directory (newest first), following renames for a file."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File or directory; the whole workspace when omitted."},
                "max_count": {"type": "integer", "minimum": 1, "maximum": MAX_HISTORY_COUNT, "default": DEFAULT_HISTORY_COUNT},
                "with_files": {"type": "boolean", "description": "Also list each commit's changed files.", "default": false},
            },
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let path = match args.get("path") {
            None | Some(Value::Null) => ".".to_owned(),
            Some(Value::String(path)) if !path.is_empty() => path.clone(),
            Some(_) => return Err("`path` must be a non-empty string".to_owned()),
        };
        check_read_path(ctx, &path)?;
        let max = count_arg(args, DEFAULT_HISTORY_COUNT, MAX_HISTORY_COUNT)?;
        let with_files = bool_arg(args, "with_files", false)?;
        // `--follow` tracks renames, and git allows it only for a single file.
        let follow = path != "." && ctx.workspace.join(&path).is_file();
        let options: &[&str] = if follow { &["--follow"] } else { &[] };
        let (commits, more) = read_log(ctx, options, &["--", &path], max, with_files)?;
        let structured = json!({
            "path": path,
            "count": commits.len(),
            "more": more,
            "commits": commits,
        });
        Ok(redacted_output(
            ctx,
            &render(&commits, more),
            Some(structured),
        ))
    }
}

/// `git_log_range`: the commits and changed files between two revisions.
#[derive(Debug)]
pub struct GitLogRangeTool;

impl McpTool for GitLogRangeTool {
    fn name(&self) -> &'static str {
        "git_log_range"
    }
    fn description(&self) -> &'static str {
        "List the commits in from..to (default to = HEAD) with the files they changed and diff totals."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "description": "Start revision (excluded), e.g. main, v1.0, HEAD~5."},
                "to": {"type": "string", "description": "End revision (included).", "default": "HEAD"},
                "max_count": {"type": "integer", "minimum": 1, "maximum": MAX_RANGE_COUNT, "default": DEFAULT_RANGE_COUNT},
                "with_files": {"type": "boolean", "default": true},
            },
            "required": ["from"],
            "additionalProperties": false,
        })
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let from = revision_arg(args, "from", None)?;
        let to = revision_arg(args, "to", Some("HEAD"))?;
        for rev in [&from, &to] {
            resolve_commit(ctx, rev)?;
        }
        let max = count_arg(args, DEFAULT_RANGE_COUNT, MAX_RANGE_COUNT)?;
        let with_files = bool_arg(args, "with_files", true)?;
        let range = format!("{from}..{to}");
        let (commits, more) = read_log(
            ctx,
            &[],
            &["--end-of-options", &range, "--", "."],
            max,
            with_files,
        )?;

        // Totals and changed files of the whole range, limited to the workspace.
        let diff = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--relative",
        ];
        let mut shortstat_args = diff.to_vec();
        shortstat_args.extend(["--shortstat", "--end-of-options", &from, &to, "--", "."]);
        let stat: DiffStat =
            parse_shortstat(&lossy(&run_git(ctx, &shortstat_args, LOG_LIMIT_BYTES)?));
        let mut names_args = diff.to_vec();
        names_args.extend([
            "--name-only",
            "-z",
            "--end-of-options",
            &from,
            &to,
            "--",
            ".",
        ]);
        let names_raw = run_git(ctx, &names_args, LOG_LIMIT_BYTES)?;
        let changed: Vec<String> = names_raw
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(lossy)
            .collect();

        let mut text = format!(
            "{} commit(s) in {range}; {} file(s) changed, +{} -{}\n",
            commits.len(),
            stat.files_changed,
            stat.insertions,
            stat.deletions
        );
        text.push_str(&render(&commits, more));
        let structured = json!({
            "from": from,
            "to": to,
            "count": commits.len(),
            "more": more,
            "commits": commits,
            "files_changed": stat.files_changed,
            "insertions": stat.insertions,
            "deletions": stat.deletions,
            "changed_files": changed.iter().take(LIST_LIMIT).collect::<Vec<_>>(),
            "changed_files_count": changed.len(),
        });
        Ok(redacted_output(ctx, &text, Some(structured)))
    }
}

fn revision_arg(args: &Value, key: &str, default: Option<&str>) -> Result<String, String> {
    let rev = match (args.get(key), default) {
        (None | Some(Value::Null), Some(default)) => default.to_owned(),
        (Some(Value::String(rev)), _) => rev.trim().to_owned(),
        (None | Some(Value::Null), None) => return Err(format!("`{key}` is required")),
        (Some(_), _) => return Err(format!("`{key}` must be a string")),
    };
    if is_plain_revision(&rev) {
        Ok(rev)
    } else {
        Err(format!(
            "`{key}` must be a branch, tag, hash or relative revision such as HEAD~3"
        ))
    }
}

/// Fails unless `rev` names a commit in this repository.
fn resolve_commit(ctx: &ToolContext, rev: &str) -> Result<(), String> {
    let spec = format!("{rev}^{{commit}}");
    run_git(
        ctx,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
        4096,
    )
    .map(drop)
    .map_err(|_| format!("unknown revision `{rev}`"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::super::test_support::{commit_files_with_message, git, init_repo};
    use super::*;

    #[test]
    fn log_records_parse_with_and_without_files() {
        let text = "\u{1e}aaaa\u{1f}aa\u{1f}Ann\u{1f}2026-10-22T10:00:00+05:30\u{1f}fix add\n\ncalc.py\ntest_calc.py\n\
                    \u{1e}bbbb\u{1f}bb\u{1f}Bo\u{1f}2026-10-21T09:00:00+05:30\u{1f}init\n\nREADME.md\n";
        let commits = parse_log(text, true);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short, "aa");
        assert_eq!(commits[0].subject, "fix add");
        assert_eq!(commits[0].files, ["calc.py", "test_calc.py"]);
        assert_eq!(commits[0].files_count, Some(2));
        let bare = parse_log(
            "\u{1e}cc\u{1f}c\u{1f}C\u{1f}2026\u{1f}msg with \u{1f} separator\n",
            false,
        );
        assert_eq!(bare[0].subject, "msg with ");
        assert!(bare[0].files.is_empty());
    }

    #[test]
    fn only_plain_revisions_are_accepted() {
        for ok in [
            "main",
            "HEAD",
            "HEAD~3",
            "v1.0",
            "a1b2c3d",
            "origin/main",
            "HEAD^2",
            "@{1}",
        ] {
            assert!(is_plain_revision(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--output=x",
            "a..b",
            "HEAD:README.md",
            "main; rm -rf x",
            "a b",
        ] {
            assert!(!is_plain_revision(bad), "{bad}");
        }
    }

    #[test]
    fn history_lists_the_commits_of_a_file_across_a_rename() {
        let Some(dir) = init_repo() else { return };
        commit_files_with_message(dir.path(), &[("a.txt", "1\n2\n3\n")], "add a");
        commit_files_with_message(dir.path(), &[("a.txt", "1\n2\n3\n4\n")], "change a");
        // A pure rename, so git's rename detection links the two names.
        git(dir.path(), &["mv", "a.txt", "b.txt"]);
        commit_files_with_message(dir.path(), &[], "rename to b");
        let ctx = test_context(dir.path());

        let out = GitHistoryTool
            .call(&json!({"path": "b.txt", "with_files": true}), &ctx)
            .unwrap();
        let structured = out.structured.unwrap();
        let subjects: Vec<&str> = structured["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["subject"].as_str().unwrap())
            .collect();
        assert_eq!(subjects, ["rename to b", "change a", "add a"]);
        assert_eq!(structured["more"], json!(false));

        let capped = GitHistoryTool.call(&json!({"max_count": 1}), &ctx).unwrap();
        assert_eq!(capped.structured.unwrap()["more"], json!(true));
    }

    #[test]
    fn history_refuses_paths_the_read_policy_denies() {
        let Some(dir) = init_repo() else { return };
        let ctx = test_context(dir.path());
        assert!(GitHistoryTool.call(&json!({"path": "../x"}), &ctx).is_err());
        assert!(GitHistoryTool.call(&json!({"path": ".env"}), &ctx).is_err());
    }

    #[test]
    fn log_range_lists_commits_files_and_totals_and_redacts_subjects() {
        let Some(dir) = init_repo() else { return };
        commit_files_with_message(dir.path(), &[("a.txt", "1\n")], "one");
        commit_files_with_message(
            dir.path(),
            &[("b.txt", "2\n")],
            "two API_KEY=sk-live-0123456789abcdefghij",
        );
        let ctx = test_context(dir.path());

        let out = GitLogRangeTool
            .call(&json!({"from": "HEAD~2"}), &ctx)
            .unwrap();
        assert!(
            !out.text.contains("sk-live-0123456789abcdefghij"),
            "{}",
            out.text
        );
        let structured = out.structured.unwrap();
        assert_eq!(structured["count"], json!(2));
        assert_eq!(structured["files_changed"], json!(2));
        assert_eq!(structured["changed_files"], json!(["a.txt", "b.txt"]));
        assert!(
            !structured
                .to_string()
                .contains("sk-live-0123456789abcdefghij")
        );
    }

    #[test]
    fn log_range_rejects_options_paths_and_unknown_revisions() {
        let Some(dir) = init_repo() else { return };
        let ctx = test_context(dir.path());
        for from in ["--output=x", "HEAD:README.md", "a..b"] {
            assert!(
                GitLogRangeTool.call(&json!({"from": from}), &ctx).is_err(),
                "{from}"
            );
        }
        let err = GitLogRangeTool
            .call(&json!({"from": "no-such-branch"}), &ctx)
            .unwrap_err();
        assert!(err.contains("unknown revision"), "{err}");
    }
}
