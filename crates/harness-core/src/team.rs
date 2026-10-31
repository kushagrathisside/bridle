//! Multi-agent work in one chat turn (ADR-0011): a planner splits the request into steps, a worker does each step with the
//! same tools and the same policy as the main agent, and a verifier runs the checks and reports.
//!
//! This module is the pure part: the roles, the planner's answer and its limits, and the text each role is given. The
//! turn itself is in `harness-cli::chat`. The verifier informs only: its report is shown, and whether the task is done is
//! still decided by the orchestrator's rules, not by the verifier.

use serde_json::Value;

/// The most steps a plan may have. A longer plan is refused, so the planner cannot start an unbounded run.
pub const MAX_STEPS: usize = 5;
/// The longest step kept, in characters. A longer step is cut.
pub const MAX_STEP_CHARS: usize = 300;
/// The most tokens the planner may write.
pub const PLANNER_MAX_TOKENS: u32 = 400;

/// The roles of a team run. The names are the ones the journal records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Planner,
    Worker,
    Verifier,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Planner => "planner",
            Role::Worker => "worker",
            Role::Verifier => "verifier",
        }
    }
}

/// The instruction for the planner. It asks for one JSON object and nothing else.
pub fn planner_system() -> String {
    format!(
        "You plan a coding task in a repository. Split the request into at most {MAX_STEPS} short, concrete steps, \
         in the order they must happen. Each step must be something one agent can do with the repository's tools. \
         Reply with one JSON object and no other text: {{\"steps\": [\"<step>\", ...]}}."
    )
}

/// A checked plan: between one and [`MAX_STEPS`] non-empty steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<String>,
}

/// Why a planner answer was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NotJson,
    NotAnObject,
    MissingSteps,
    NoSteps,
    TooManySteps(usize),
    EmptyStep,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::NotJson => f.write_str("the plan is not JSON"),
            PlanError::NotAnObject => f.write_str("the plan is not a JSON object"),
            PlanError::MissingSteps => f.write_str("the plan has no steps list"),
            PlanError::NoSteps => f.write_str("the plan has no steps"),
            PlanError::TooManySteps(count) => {
                write!(
                    f,
                    "the plan has {count} steps; at most {MAX_STEPS} are allowed"
                )
            }
            PlanError::EmptyStep => f.write_str("the plan has an empty step"),
        }
    }
}

/// Parses the planner's answer. A Markdown code fence around the object is allowed, and nothing else is.
pub fn parse_plan(text: &str) -> Result<Plan, PlanError> {
    let value: Value =
        serde_json::from_str(strip_fence(text.trim())).map_err(|_| PlanError::NotJson)?;
    let object = value.as_object().ok_or(PlanError::NotAnObject)?;
    let list = object
        .get("steps")
        .and_then(Value::as_array)
        .ok_or(PlanError::MissingSteps)?;
    if list.is_empty() {
        return Err(PlanError::NoSteps);
    }
    if list.len() > MAX_STEPS {
        return Err(PlanError::TooManySteps(list.len()));
    }
    let mut steps = Vec::with_capacity(list.len());
    for item in list {
        let step = clean_step(item.as_str().unwrap_or_default());
        if step.is_empty() {
            return Err(PlanError::EmptyStep);
        }
        steps.push(step);
    }
    Ok(Plan { steps })
}

/// The plan for a request the planner could not plan: the whole request, as one step.
pub fn one_step(request: &str) -> Plan {
    let step = clean_step(request);
    Plan {
        steps: vec![if step.is_empty() {
            "answer the request".to_owned()
        } else {
            step
        }],
    }
}

/// The text given to the worker for one step, numbered so the worker knows where it is in the run.
pub fn worker_text(index: usize, total: usize, step: &str) -> String {
    format!("Step {index} of {total}: {step}")
}

/// The text given to the verifier after the workers. It runs the checks and reports; it changes no file.
pub fn verifier_text() -> &'static str {
    "Check the work of this run. Run the project's checks with run_tests or run_verification when they are \
     configured, and say which passed and which failed. Do not change any file."
}

/// Removes control characters, collapses the whitespace around the text, and cuts it to [`MAX_STEP_CHARS`].
fn clean_step(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    clean.trim().chars().take(MAX_STEP_CHARS).collect()
}

fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_of_three_steps_is_read_in_order() {
        let plan = parse_plan(r#"{"steps": ["read the parser", "add the test", "run the tests"]}"#)
            .unwrap();
        assert_eq!(
            plan.steps,
            ["read the parser", "add the test", "run the tests"]
        );
    }

    #[test]
    fn a_fenced_plan_is_allowed() {
        let plan = parse_plan("```json\n{\"steps\": [\"fix the typo\"]}\n```").unwrap();
        assert_eq!(plan.steps, ["fix the typo"]);
    }

    #[test]
    fn a_plan_outside_the_limits_is_refused() {
        assert_eq!(parse_plan("not json"), Err(PlanError::NotJson));
        assert_eq!(parse_plan("[1]"), Err(PlanError::NotAnObject));
        assert_eq!(parse_plan("{}"), Err(PlanError::MissingSteps));
        assert_eq!(parse_plan("{\"steps\": []}"), Err(PlanError::NoSteps));
        assert_eq!(
            parse_plan("{\"steps\": [\"a\", \" \"]}"),
            Err(PlanError::EmptyStep)
        );
        assert_eq!(
            parse_plan("{\"steps\": [\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]}"),
            Err(PlanError::TooManySteps(6))
        );
    }

    #[test]
    fn control_characters_are_removed_and_a_long_step_is_cut() {
        let long = "x".repeat(MAX_STEP_CHARS + 50);
        let plan = parse_plan(&format!("{{\"steps\": [\"a\\u0007b\", \"{long}\"]}}")).unwrap();
        assert_eq!(plan.steps[0], "ab");
        assert_eq!(plan.steps[1].chars().count(), MAX_STEP_CHARS);
    }

    #[test]
    fn a_request_the_planner_could_not_plan_is_one_step() {
        assert_eq!(one_step("  fix the typo  ").steps, ["fix the typo"]);
        assert_eq!(one_step("   ").steps, ["answer the request"]);
    }

    #[test]
    fn the_worker_is_told_where_it_is_in_the_run() {
        assert_eq!(
            worker_text(2, 3, "run the tests"),
            "Step 2 of 3: run the tests"
        );
        assert_eq!(Role::Verifier.name(), "verifier");
    }
}
