# MCP tools

Bridle serves its own tools to the agent over the Model Context Protocol. OpenCode starts `bridle mcp` from its generated
configuration, and each tool appears to the agent as `bridle_<name>`. Each tool call still passes the policy first,
like any other call.

The table lists the tools in the order the server reports them. The class is the policy class the tool is judged by.

| Tool | Class | What it does |
|---|---|---|
| `git_status` | READ_ONLY | Shows the branch and the staged, unstaged, untracked and conflicted paths of the workspace. |
| `git_diff` | READ_ONLY | Shows a summary and the patch of unstaged (or staged) changes, optionally limited to paths. |
| `git_history` | READ_ONLY | Lists the commits that changed a file or directory, newest first, following renames for a file. |
| `git_log_range` | READ_ONLY | Lists the commits in `from..to` (default `to` is `HEAD`), with the files they changed and diff totals. |
| `run_verification` | EXECUTE_SAFE | Runs the configured verification steps in order and reports which passed. |
| `run_tests` | EXECUTE_SAFE | Runs the configured test steps, optionally only for given test files, node ids or test names. |
| `run_linter` | EXECUTE_SAFE | Runs the configured lint steps and reports the findings. |
| `run_typechecker` | EXECUTE_SAFE | Runs the configured type-check steps and reports the errors. |
| `parse_test_failure` | READ_ONLY | Extracts failing tests, compiler errors, type errors and lint findings (name, file, line, message) from command output. |
| `repo_map` | READ_ONLY | Maps the tracked files: busiest directories, languages with parseable source, and the key build files. Secret files are never listed. |
| `file_summary` | READ_ONLY | Summarises one source file: language, line count, definitions with their lines, and imports. |
| `symbol_search` | READ_ONLY | Finds where a function, class, struct, type or module is defined. Matches exact, prefix or contains. |
| `find_references` | READ_ONLY | Lists the lines that use an identifier. The search is syntactic: comments and strings are excluded, and scopes and types are not resolved. |
| `dependency_search` | READ_ONLY | Lists the dependencies declared by `Cargo.toml`, `pyproject.toml`, `requirements*.txt` and `package.json`, and the source files that import one. |
| `inspect_artifact` | READ_ONLY | Summarises a result file: JSON shape, JSON Lines or CSV field statistics, or the tail of a text log. |
| `summarize_experiment` | READ_ONLY | Summarises an experiment folder: its files, its config (secrets redacted), and the final value of each metric. |
| `compare_experiments` | READ_ONLY | Compares two experiment folders: final metric values with their differences, and config keys that differ. |
| `run_command` | EXECUTE_SAFE | Runs one program on the PATH with plain arguments and no shell syntax. The policy checks it first. |
| `inspect_config` | READ_ONLY | Shows the effective configuration (secrets redacted) and its validation findings. |
| `harness_info` | READ_ONLY | Describes the harness: version, workspace, profile, model slots and verification steps. |

## Tools that depend on configuration

| Tool | Class | When it appears |
|---|---|---|
| `adapter_<name>` | EXECUTE_SAFE | One tool per `[[tools.adapters]]` entry in the machine configuration. A repository cannot add tools. |
| `task_status` | READ_ONLY | Only when `workflow.task_status` is `true`. Off by default. |
| `subagent` | EXECUTE_SAFE | Only when `subagents.enabled` is `true`. Off by default. |
| `command_start` | EXECUTE_SAFE (each command judged as `run_command`) | Only when `workflow.background_commands` is `true`. Starts a program in the background; returns an id. See [Notices and background commands](../guide/background-commands.md). |
| `command_status`, `command_stop` | READ_ONLY | The same condition. Read a background command's state and output; end one that Bridle started (always allowed, like `browser_close`). |

| `web_search`, `web_open`, `web_download` | NETWORK | Only when `[web] enabled`. See [Web research](../guide/web-research.md). Each is refused inside the tool unless `security.network = "allow"`. |
| `document_inspect` | READ_ONLY | Only when `[web] enabled`. Reads a saved download by source id, or a file in the project: text, a PDF's pages and text, a zip, tar or tar.gz listing. |
| `browser_navigate`, `browser_snapshot`, `browser_action` | NETWORK | Only when `[web] enabled` and `[web.browser] enabled`. A managed Chromium; see [The managed browser](../guide/web-browser.md). Refused inside the tool unless `security.network = "allow"`. |
| `browser_close` | READ_ONLY | The same condition. Closes a session; always allowed so a session can be freed. |
| `window_screenshot` | EXECUTE_SAFE or EXECUTE_RISKY (as the program) | Only when `[desktop] screenshots_enabled`, and only on Windows. Starts a program under the `run_command` rules, takes a picture of its window only, stops it. See [Window screenshots](../guide/window-screenshots.md). |

Any tool name the server does not know is blocked, including the names a general browser server would offer (`browser_evaluate`,
`browser_run_code`, a raw DevTools command, `browser_install`). The browser tools have no way to run a script, choose a file path or change
how Chromium starts.

## Limits

- The server handles one request at a time, so a slow tool holds the others until its time limit expires.
- The code tools are syntactic. They do not resolve imports, overloads or types.
