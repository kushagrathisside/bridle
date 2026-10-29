//! Failure parsing: test, compiler, type-checker and linter output turned into
//! structured failures, so the model reads "test_add failed at tests/test_calc.py:5:
//! assert -1 == 3" instead of scrolling a log.
//!
//! Pure functions over text. Every parser runs over the whole output; results are merged and
//! deduplicated, and capped at [`MAX_FAILURES`]. Formats covered: pytest, unittest and Python
//! tracebacks; `cargo test` and rustc; GoogleTest and CTest; GCC/Clang, MSVC and tsc; mypy,
//! ruff/flake8 and pyright; `go test`.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// The most failures reported for one output.
pub const MAX_FAILURES: usize = 50;
/// The longest message kept per failure.
const MAX_MESSAGE: usize = 300;

/// What failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A test assertion failed.
    TestFailure,
    /// A test errored (exception outside an assertion, collection error, panic).
    TestError,
    /// The code did not compile.
    CompileError,
    /// A type checker reported an error.
    TypeError,
    /// A linter reported a finding.
    LintFinding,
}

/// One parsed failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// The tool whose output format matched (`pytest`, `cargo`, `gcc`, ...).
    pub tool: &'static str,
    pub kind: FailureKind,
    /// Test name or node id, when the failure is a test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The first line of the error, trimmed and capped.
    pub message: String,
}

/// Everything found in one output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FailureReport {
    pub failures: Vec<Failure>,
    /// Failures found beyond [`MAX_FAILURES`].
    pub omitted: usize,
    /// Summary lines such as `1 failed, 1 passed in 0.12s`.
    pub summaries: Vec<String>,
}

/// Parses `output` with every known format.
pub fn parse_failures(output: &str) -> FailureReport {
    let lines: Vec<&str> = output.lines().collect();
    let mut found = Vec::new();
    found.extend(pytest(&lines));
    found.extend(unittest(&lines));
    found.extend(cargo_tests(&lines));
    found.extend(rustc(&lines));
    found.extend(gtest(&lines));
    found.extend(ctest(&lines));
    found.extend(go_test(&lines));
    found.extend(located_diagnostics(&lines));
    if found.is_empty() {
        found.extend(python_traceback(&lines));
    }

    let mut failures: Vec<Failure> = Vec::new();
    for failure in found {
        if !failures.iter().any(|known| same_failure(known, &failure)) {
            failures.push(failure);
        }
    }
    let omitted = failures.len().saturating_sub(MAX_FAILURES);
    failures.truncate(MAX_FAILURES);
    FailureReport {
        failures,
        omitted,
        summaries: summaries(&lines),
    }
}

/// Two reports of one failure: the same test, or the same location and message.
fn same_failure(a: &Failure, b: &Failure) -> bool {
    match (&a.name, &b.name) {
        (Some(x), Some(y)) => x == y,
        _ => a.file.is_some() && a.file == b.file && a.line == b.line && a.message == b.message,
    }
}

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

fn message(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_MESSAGE) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn number(text: Option<regex::Match<'_>>) -> Option<u32> {
    text.and_then(|m| m.as_str().parse().ok())
}

// ---------------------------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------------------------

/// `FAILED tests/test_x.py::test_y - AssertionError: ...` (and `ERROR`), from pytest's short
/// summary, with the failing line taken from the test's section (`tests/test_x.py:5: Error`).
static PYTEST_SUMMARY: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(FAILED|ERROR) ([^\s(]\S*?)(?: - (.*))?$"));
static PYTEST_SECTION: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^_{3,} (?:ERROR (?:at \w+ of |collecting ))?(\S+) _{3,}$"));
static PYTEST_LOCATION: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(\S+\.py):(\d+): (.*)$"));

fn pytest(lines: &[&str]) -> Vec<Failure> {
    let (Some(summary), Some(section), Some(location)) =
        (&*PYTEST_SUMMARY, &*PYTEST_SECTION, &*PYTEST_LOCATION)
    else {
        return Vec::new();
    };
    // The last `file:line: message` of each section is where that test failed.
    let mut current: Option<String> = None;
    let mut located: HashMap<String, (String, u32, String)> = HashMap::new();
    for line in lines {
        if let Some(caps) = section.captures(line) {
            current = caps.get(1).map(|m| m.as_str().to_owned());
        } else if let (Some(name), Some(caps)) = (&current, location.captures(line)) {
            let line_no = number(caps.get(2)).unwrap_or(0);
            located.insert(
                name.clone(),
                (caps[1].to_owned(), line_no, caps[3].to_owned()),
            );
        }
    }
    lines
        .iter()
        .filter_map(|line| summary.captures(line))
        .map(|caps| {
            let node = caps[2].to_owned();
            let file = node.split("::").next().map(str::to_owned);
            // Section headers name `Class.test` for `file::Class::test`.
            let key = node.split("::").skip(1).collect::<Vec<_>>().join(".");
            let at = located.get(&key).or_else(|| located.get(&node));
            Failure {
                tool: "pytest",
                kind: if &caps[1] == "FAILED" {
                    FailureKind::TestFailure
                } else {
                    FailureKind::TestError
                },
                name: Some(node.clone()),
                file: at.map(|(f, _, _)| f.clone()).or(file),
                line: at.map(|(_, l, _)| *l).filter(|l| *l > 0),
                column: None,
                message: message(
                    caps.get(3)
                        .map(|m| m.as_str())
                        .or(at.map(|(_, _, m)| m.as_str()))
                        .unwrap_or("failed"),
                ),
            }
        })
        .collect()
}

/// `FAIL: test_add (test_calc.CalcTests.test_add)` blocks with their traceback.
static UNITTEST_HEADER: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(FAIL|ERROR): (\S+) \(([^)]*)\)"));
static PY_FILE_LINE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r#"^\s*File "([^"]+)", line (\d+)"#));
static PY_EXCEPTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(
        r"^([A-Za-z_][\w.]*(?:Error|Exception|Exit|Interrupt|Warning)|AssertionError)(?::\s*(.*))?$",
    )
});

fn unittest(lines: &[&str]) -> Vec<Failure> {
    let (Some(header), Some(file_line), Some(exception)) =
        (&*UNITTEST_HEADER, &*PY_FILE_LINE, &*PY_EXCEPTION)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(caps) = header.captures(line) else {
            continue;
        };
        let block: Vec<&str> = lines[index + 1..]
            .iter()
            .take_while(|l| !l.starts_with("====") && !header.is_match(l))
            .copied()
            .collect();
        let at = block.iter().rev().find_map(|l| file_line.captures(l));
        let text = block
            .iter()
            .rev()
            .find_map(|l| exception.captures(l).map(|c| c[0].to_owned()))
            .unwrap_or_else(|| "failed".to_owned());
        let qualified = caps[3].to_owned();
        let name = if qualified.ends_with(&caps[2]) {
            qualified
        } else {
            format!("{qualified}.{}", &caps[2])
        };
        out.push(Failure {
            tool: "unittest",
            kind: if &caps[1] == "FAIL" {
                FailureKind::TestFailure
            } else {
                FailureKind::TestError
            },
            name: Some(name),
            file: at.as_ref().map(|c| c[1].to_owned()),
            line: at.as_ref().and_then(|c| number(c.get(2))),
            column: None,
            message: message(&text),
        });
    }
    out
}

/// A bare Python traceback (a script or import that crashed): the deepest frame and the exception.
fn python_traceback(lines: &[&str]) -> Vec<Failure> {
    let (Some(file_line), Some(exception)) = (&*PY_FILE_LINE, &*PY_EXCEPTION) else {
        return Vec::new();
    };
    let Some(start) = lines
        .iter()
        .rposition(|l| l.starts_with("Traceback (most recent call last):"))
    else {
        return Vec::new();
    };
    let block = &lines[start..];
    let at = block.iter().rev().find_map(|l| file_line.captures(l));
    let Some(text) = block
        .iter()
        .rev()
        .find_map(|l| exception.captures(l).map(|c| c[0].to_owned()))
    else {
        return Vec::new();
    };
    vec![Failure {
        tool: "python",
        kind: FailureKind::TestError,
        name: None,
        file: at.as_ref().map(|c| c[1].to_owned()),
        line: at.as_ref().and_then(|c| number(c.get(2))),
        column: None,
        message: message(&text),
    }]
}

// ---------------------------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------------------------

static CARGO_FAILED: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^test (\S+) \.\.\. FAILED$"));
/// `thread 'name' panicked at src/x.rs:10:5:` (current) or `panicked at 'msg', src/x.rs:10:5`.
static RUST_PANIC: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r"^thread '([^']+)'(?: \(\d+\))? panicked at (?:'(.*)', )?([^:\s]+):(\d+):(\d+):?(.*)$")
});

fn cargo_tests(lines: &[&str]) -> Vec<Failure> {
    let (Some(failed), Some(panic)) = (&*CARGO_FAILED, &*RUST_PANIC) else {
        return Vec::new();
    };
    let mut panics: HashMap<String, (String, u32, u32, String)> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(caps) = panic.captures(line) {
            // The message is inline (old format) or on the next non-empty line (new format).
            let inline = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            let trailing = caps.get(6).map(|m| m.as_str().trim()).unwrap_or_default();
            let text = if !inline.is_empty() {
                inline.to_owned()
            } else if !trailing.is_empty() {
                trailing.to_owned()
            } else {
                lines
                    .get(index + 1..)
                    .and_then(|rest| rest.iter().find(|l| !l.trim().is_empty()))
                    .map_or_else(|| "panicked".to_owned(), |l| (*l).to_owned())
            };
            panics.insert(
                caps[1].to_owned(),
                (
                    caps[3].to_owned(),
                    number(caps.get(4)).unwrap_or(0),
                    number(caps.get(5)).unwrap_or(0),
                    text,
                ),
            );
        }
    }
    lines
        .iter()
        .filter_map(|line| failed.captures(line))
        .map(|caps| {
            let name = caps[1].to_owned();
            let at = panics.get(&name);
            Failure {
                tool: "cargo",
                kind: FailureKind::TestFailure,
                name: Some(name),
                file: at.map(|(f, ..)| f.clone()),
                line: at.map(|(_, l, ..)| *l).filter(|l| *l > 0),
                column: at.map(|(_, _, c, _)| *c).filter(|c| *c > 0),
                message: message(at.map_or("failed", |(.., m)| m.as_str())),
            }
        })
        .collect()
}

/// `error[E0425]: cannot find value` followed by `  --> src/main.rs:2:13`.
static RUSTC_ERROR: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^error(?:\[(E\d{4})\])?: (.+)$"));
static RUSTC_ARROW: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^\s*--> ([^:]+):(\d+):(\d+)$"));

fn rustc(lines: &[&str]) -> Vec<Failure> {
    let (Some(error), Some(arrow)) = (&*RUSTC_ERROR, &*RUSTC_ARROW) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(caps) = error.captures(line) else {
            continue;
        };
        let text = &caps[2];
        // Cargo's own wrap-up lines are not diagnostics.
        if text.starts_with("could not compile") || text.starts_with("aborting due to") {
            continue;
        }
        let Some(at) = lines
            .get(index + 1..index + 4)
            .and_then(|next| next.iter().find_map(|l| arrow.captures(l)))
        else {
            continue;
        };
        let code = caps
            .get(1)
            .map(|c| format!("{} ", c.as_str()))
            .unwrap_or_default();
        out.push(Failure {
            tool: "rustc",
            kind: FailureKind::CompileError,
            name: None,
            file: Some(at[1].to_owned()),
            line: number(at.get(2)),
            column: number(at.get(3)),
            message: message(&format!("{code}{text}")),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// C++
// ---------------------------------------------------------------------------------------------

static GTEST_FAILED: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^\[  FAILED  \] (\S+\.\S+)(?: \(\d+ ms\))?$"));
static GTEST_RUN: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"^\[ RUN      \] (\S+)$"));
static GTEST_LOCATION: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(.+?)[:(](\d+)\)?: (?:Failure|error)$"));

fn gtest(lines: &[&str]) -> Vec<Failure> {
    let (Some(failed), Some(run), Some(location)) = (&*GTEST_FAILED, &*GTEST_RUN, &*GTEST_LOCATION)
    else {
        return Vec::new();
    };
    let mut current: Option<String> = None;
    let mut located: HashMap<String, (String, u32, String)> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(caps) = run.captures(line) {
            current = Some(caps[1].to_owned());
        } else if let (Some(name), Some(caps)) = (&current, location.captures(line))
            && !located.contains_key(name)
        {
            let text = lines
                .get(index + 1..)
                .and_then(|rest| rest.iter().find(|l| !l.trim().is_empty()))
                .map_or("Failure", |l| l.trim());
            located.insert(
                name.clone(),
                (
                    caps[1].to_owned(),
                    number(caps.get(2)).unwrap_or(0),
                    text.to_owned(),
                ),
            );
        }
    }
    let mut seen = Vec::new();
    lines
        .iter()
        .filter_map(|line| failed.captures(line))
        .filter_map(|caps| {
            // The summary repeats each failed test; report it once.
            let name = caps[1].to_owned();
            if seen.contains(&name) {
                return None;
            }
            seen.push(name.clone());
            let at = located.get(&name);
            Some(Failure {
                tool: "gtest",
                kind: FailureKind::TestFailure,
                name: Some(name),
                file: at.map(|(f, ..)| f.clone()),
                line: at.map(|(_, l, _)| *l).filter(|l| *l > 0),
                column: None,
                message: message(at.map_or("failed", |(.., m)| m.as_str())),
            })
        })
        .collect()
}

/// `  1 - unit_tests (Failed)` under CTest's "The following tests FAILED:".
static CTEST_FAILED: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r"^\s*\d+ - (\S+) \((Failed|Timeout|SEGFAULT|Exception|Child aborted|[A-Za-z ]+)\)$")
});

fn ctest(lines: &[&str]) -> Vec<Failure> {
    let Some(failed) = &*CTEST_FAILED else {
        return Vec::new();
    };
    let Some(start) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("The following tests FAILED"))
    else {
        return Vec::new();
    };
    lines[start + 1..]
        .iter()
        .map_while(|line| failed.captures(line))
        .map(|caps| Failure {
            tool: "ctest",
            kind: FailureKind::TestFailure,
            name: Some(caps[1].to_owned()),
            file: None,
            line: None,
            column: None,
            message: message(&caps[2]),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------------------------

static GO_FAIL: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^\s*--- FAIL: (\S+) \([\d.]+s\)$"));
static GO_LOCATION: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^\s+(\S+_test\.go):(\d+): (.+)$"));

fn go_test(lines: &[&str]) -> Vec<Failure> {
    let (Some(fail), Some(location)) = (&*GO_FAIL, &*GO_LOCATION) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(caps) = fail.captures(line) else {
            continue;
        };
        let at = lines
            .get(index + 1..)
            .and_then(|rest| rest.iter().map_while(|l| location.captures(l)).next());
        out.push(Failure {
            tool: "go",
            kind: FailureKind::TestFailure,
            name: Some(caps[1].to_owned()),
            file: at.as_ref().map(|c| c[1].to_owned()),
            line: at.as_ref().and_then(|c| number(c.get(2))),
            column: None,
            message: message(
                at.as_ref()
                    .map_or("failed", |c| c.get(3).map_or("failed", |m| m.as_str())),
            ),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Diagnostics with a location: compilers, type checkers, linters
// ---------------------------------------------------------------------------------------------

/// GCC/Clang/mypy: `src/a.cpp:10:5: error: msg` or `src/x.py:10: error: msg [code]`.
static GNU_DIAGNOSTIC: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(.+?):(\d+):(?:(\d+):)? (?:fatal )?error: (.+)$"));
/// MSVC and tsc: `src\a.cpp(10): error C2065: msg`, `src/a.ts(10,5): error TS2322: msg`.
static PAREN_DIAGNOSTIC: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r"^\s*(.+?)\((\d+)(?:,(\d+))?\)\s*: (?:fatal )?error ([A-Z]+\d+): (.+)$")
});
/// ruff/flake8: `src/x.py:10:5: F401 [*] msg` (a code instead of `error:`).
static LINT_DIAGNOSTIC: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^(.+?\.\w+):(\d+):(\d+): ([A-Z]+\d+) (?:\[\*\] )?(.+)$"));
/// pyright: `  /x/a.py:10:5 - error: msg`.
static PYRIGHT_DIAGNOSTIC: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^\s*(.+?\.pyi?):(\d+):(\d+) - error: (.+)$"));

fn located_diagnostics(lines: &[&str]) -> Vec<Failure> {
    let mut out = Vec::new();
    for line in lines {
        if let Some(caps) = PAREN_DIAGNOSTIC.as_ref().and_then(|r| r.captures(line)) {
            let code = &caps[4];
            let (tool, kind) = if code.starts_with("TS") {
                ("tsc", FailureKind::TypeError)
            } else {
                ("msvc", FailureKind::CompileError)
            };
            out.push(Failure {
                tool,
                kind,
                name: None,
                file: Some(caps[1].to_owned()),
                line: number(caps.get(2)),
                column: number(caps.get(3)),
                message: message(&format!("{code} {}", &caps[5])),
            });
        } else if let Some(caps) = PYRIGHT_DIAGNOSTIC.as_ref().and_then(|r| r.captures(line)) {
            out.push(Failure {
                tool: "pyright",
                kind: FailureKind::TypeError,
                name: None,
                file: Some(caps[1].to_owned()),
                line: number(caps.get(2)),
                column: number(caps.get(3)),
                message: message(&caps[4]),
            });
        } else if let Some(caps) = GNU_DIAGNOSTIC.as_ref().and_then(|r| r.captures(line)) {
            let file = &caps[1];
            let python = file.ends_with(".py") || file.ends_with(".pyi");
            out.push(Failure {
                tool: if python { "mypy" } else { "gcc" },
                kind: if python {
                    FailureKind::TypeError
                } else {
                    FailureKind::CompileError
                },
                name: None,
                file: Some(file.to_owned()),
                line: number(caps.get(2)),
                column: number(caps.get(3)),
                message: message(&caps[4]),
            });
        } else if let Some(caps) = LINT_DIAGNOSTIC.as_ref().and_then(|r| r.captures(line)) {
            out.push(Failure {
                tool: "lint",
                kind: FailureKind::LintFinding,
                name: None,
                file: Some(caps[1].to_owned()),
                line: number(caps.get(2)),
                column: number(caps.get(3)),
                message: message(&format!("{} {}", &caps[4], &caps[5])),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Summaries
// ---------------------------------------------------------------------------------------------

static SUMMARY_LINES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // pytest: `=== 1 failed, 1 passed in 0.12s ===`
        r"^=+ (.*\d+ (?:failed|passed|error|errors|skipped).*) =+$",
        // unittest: `FAILED (failures=1)` / `Ran 2 tests in 0.001s`
        r"^(FAILED \(.*\)|Ran \d+ tests? in .*)$",
        // cargo: `test result: FAILED. 1 passed; 1 failed; ...`
        r"^(test result: .*)$",
        // gtest: `[  FAILED  ] 1 test, listed below:`, ctest: `50% tests passed, 1 tests failed out of 2`
        r"^(\[  FAILED  \] \d+ tests?, listed below:|\d+% tests passed.*)$",
        // mypy: `Found 2 errors in 1 file (checked 3 source files)`
        r"^(Found \d+ errors? in .*)$",
    ]
    .into_iter()
    .filter_map(regex)
    .collect()
});

fn summaries(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| {
            SUMMARY_LINES
                .iter()
                .find_map(|r| r.captures(line.trim()).and_then(|c| c.get(1)))
                .map(|m| m.as_str().to_owned())
        })
        .take(10)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn one(output: &str) -> Failure {
        let report = parse_failures(output);
        assert_eq!(report.failures.len(), 1, "{report:#?}");
        report.failures.into_iter().next().unwrap()
    }

    #[test]
    fn every_pattern_compiles() {
        for pattern in [
            &*PYTEST_SUMMARY,
            &*PYTEST_SECTION,
            &*PYTEST_LOCATION,
            &*UNITTEST_HEADER,
            &*PY_FILE_LINE,
            &*PY_EXCEPTION,
            &*CARGO_FAILED,
            &*RUST_PANIC,
            &*RUSTC_ERROR,
            &*RUSTC_ARROW,
            &*GTEST_FAILED,
            &*GTEST_RUN,
            &*GTEST_LOCATION,
            &*CTEST_FAILED,
            &*GO_FAIL,
            &*GO_LOCATION,
            &*GNU_DIAGNOSTIC,
            &*PAREN_DIAGNOSTIC,
            &*LINT_DIAGNOSTIC,
            &*PYRIGHT_DIAGNOSTIC,
        ] {
            assert!(pattern.is_some());
        }
        assert_eq!(SUMMARY_LINES.len(), 5);
    }

    #[test]
    fn pytest_failures_get_their_line_from_the_section() {
        let out = "\
============================= FAILURES =============================
_____________________________ test_add _____________________________

    def test_add():
>       assert add(1, 2) == 3
E       assert -1 == 3

tests/test_calc.py:5: AssertionError
_______________________ TestCalc.test_mul __________________________
tests/test_calc.py:12: AssertionError
=========================== short test summary info ===========================
FAILED tests/test_calc.py::test_add - assert -1 == 3
FAILED tests/test_calc.py::TestCalc::test_mul - AssertionError
ERROR tests/test_io.py - ModuleNotFoundError: No module named 'yaml'
==================== 2 failed, 3 passed, 1 error in 0.12s ====================";
        let report = parse_failures(out);
        let f = &report.failures;
        assert_eq!(f.len(), 3, "{f:#?}");
        assert_eq!(f[0].name.as_deref(), Some("tests/test_calc.py::test_add"));
        assert_eq!(
            (f[0].file.as_deref(), f[0].line),
            (Some("tests/test_calc.py"), Some(5))
        );
        assert_eq!(f[0].message, "assert -1 == 3");
        assert_eq!(f[1].line, Some(12));
        assert_eq!(f[2].kind, FailureKind::TestError);
        assert_eq!(report.summaries, ["2 failed, 3 passed, 1 error in 0.12s"]);
    }

    #[test]
    fn unittest_blocks_give_the_deepest_frame_and_exception() {
        let out = "\
F.
======================================================================
FAIL: test_add (test_calc.CalcTests.test_add)
----------------------------------------------------------------------
Traceback (most recent call last):
  File \"D:\\e2e\\toy\\test_calc.py\", line 6, in test_add
    self.assertEqual(add(1, 2), 3)
AssertionError: -1 != 3

----------------------------------------------------------------------
Ran 2 tests in 0.001s

FAILED (failures=1)";
        let f = one(out);
        assert_eq!(f.tool, "unittest");
        assert_eq!(f.name.as_deref(), Some("test_calc.CalcTests.test_add"));
        assert_eq!(
            (f.file.as_deref(), f.line),
            (Some("D:\\e2e\\toy\\test_calc.py"), Some(6))
        );
        assert_eq!(f.message, "AssertionError: -1 != 3");
        assert_eq!(
            parse_failures(out).summaries,
            ["Ran 2 tests in 0.001s", "FAILED (failures=1)"]
        );
    }

    #[test]
    fn cargo_test_failures_use_the_panic_location_in_both_formats() {
        let current = "\
running 2 tests
test calc::tests::add ... FAILED
test calc::tests::mul ... ok

failures:

---- calc::tests::add stdout ----

thread 'calc::tests::add' panicked at src/calc.rs:10:9:
assertion `left == right` failed
  left: -1
 right: 3

test result: FAILED. 1 passed; 1 failed; 0 ignored";
        let f = one(current);
        assert_eq!(f.name.as_deref(), Some("calc::tests::add"));
        assert_eq!(
            (f.file.as_deref(), f.line, f.column),
            (Some("src/calc.rs"), Some(10), Some(9))
        );
        assert_eq!(f.message, "assertion `left == right` failed");

        let old = "test add ... FAILED\nthread 'add' panicked at 'boom', src/lib.rs:3:5";
        let f = one(old);
        assert_eq!((f.line, f.message.as_str()), (Some(3), "boom"));
    }

    #[test]
    fn rustc_errors_take_the_arrow_location() {
        let out = "\
error[E0425]: cannot find value `x` in this scope
 --> src/main.rs:2:13
  |
2 |     println!(\"{}\", x);
  |                    ^ not found in this scope

error: could not compile `demo` (bin \"demo\") due to 1 previous error";
        let f = one(out);
        assert_eq!(f.kind, FailureKind::CompileError);
        assert_eq!(
            (f.file.as_deref(), f.line, f.column),
            (Some("src/main.rs"), Some(2), Some(13))
        );
        assert_eq!(f.message, "E0425 cannot find value `x` in this scope");
    }

    #[test]
    fn gtest_and_ctest_failures_are_reported_once() {
        let gtest = "\
[ RUN      ] CalcTest.Add
/src/calc_test.cpp:12: Failure
Expected equality of these values:
  add(1, 2)
[  FAILED  ] CalcTest.Add (0 ms)
[  FAILED  ] 1 test, listed below:
[  FAILED  ] CalcTest.Add";
        let f = one(gtest);
        assert_eq!(f.name.as_deref(), Some("CalcTest.Add"));
        assert_eq!(
            (f.file.as_deref(), f.line),
            (Some("/src/calc_test.cpp"), Some(12))
        );
        assert_eq!(f.message, "Expected equality of these values:");

        let ctest = "\
50% tests passed, 1 tests failed out of 2
The following tests FAILED:
\t  1 - unit_tests (Failed)
Errors while running CTest";
        let f = one(ctest);
        assert_eq!((f.tool, f.name.as_deref()), ("ctest", Some("unit_tests")));
    }

    #[test]
    fn compiler_type_checker_and_linter_diagnostics() {
        let out = "\
src/calc.cpp:10:5: error: 'x' was not declared in this scope
D:\\proj\\calc.cpp(10): error C2065: 'x': undeclared identifier
src/app.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.
src/calc.py:4: error: Incompatible return value type (got \"str\", expected \"int\")  [return-value]
src/calc.py:1:8: F401 [*] `os` imported but unused
  /w/src/calc.py:4:12 - error: Expression of type \"str\" cannot be assigned
Found 1 error in 1 file (checked 2 source files)";
        let report = parse_failures(out);
        let tools: Vec<&str> = report.failures.iter().map(|f| f.tool).collect();
        assert_eq!(tools, ["gcc", "msvc", "tsc", "mypy", "lint", "pyright"]);
        assert_eq!(
            report.failures[1].file.as_deref(),
            Some("D:\\proj\\calc.cpp")
        );
        assert_eq!(report.failures[2].column, Some(7));
        assert_eq!(report.failures[4].message, "F401 `os` imported but unused");
        assert_eq!(
            report.summaries,
            ["Found 1 error in 1 file (checked 2 source files)"]
        );
    }

    #[test]
    fn go_test_failures_and_bare_tracebacks() {
        let go = "--- FAIL: TestAdd (0.00s)\n    calc_test.go:12: expected 3, got -1\nFAIL";
        let f = one(go);
        assert_eq!((f.name.as_deref(), f.line), (Some("TestAdd"), Some(12)));
        assert_eq!(f.message, "expected 3, got -1");

        let crash = "\
Traceback (most recent call last):
  File \"run.py\", line 3, in <module>
    import yaml
ModuleNotFoundError: No module named 'yaml'";
        let f = one(crash);
        assert_eq!((f.tool, f.line), ("python", Some(3)));
        assert_eq!(f.message, "ModuleNotFoundError: No module named 'yaml'");
    }

    #[test]
    fn clean_output_has_no_failures_and_long_messages_are_capped() {
        assert!(parse_failures("all good\n2 passed").failures.is_empty());
        let long = format!("src/a.cpp:1:1: error: {}", "x".repeat(1000));
        assert!(one(&long).message.chars().count() <= MAX_MESSAGE + 1);
        let many: String = (0..80)
            .map(|i| format!("src/a.cpp:{i}:1: error: e{i}\n"))
            .collect();
        let report = parse_failures(&many);
        assert_eq!((report.failures.len(), report.omitted), (MAX_FAILURES, 30));
    }
}
