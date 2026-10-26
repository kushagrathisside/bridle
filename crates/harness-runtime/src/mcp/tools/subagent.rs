//! The `subagent` MCP tool (M4b): asks the small model for a checked answer about a few files, or a condensed text.
//!
//! The tool is offered only when `[subagents] enabled = true` (see `harness-cli/src/commands/mcp.rs`). It cannot edit a
//! file or run a command. Its answer is a reading of redacted inputs, and the text says so.

use harness_core::subagent::Role;
use serde_json::{Value, json};

use super::{McpTool, ToolContext, ToolOutput};
use crate::subagent::{Request, run_for_config};

/// The MCP tool name. OpenCode shows it to the model as `local-agent_subagent`.
pub const NAME: &str = "subagent";

#[derive(Debug)]
pub struct SubagentTool;

impl McpTool for SubagentTool {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        "Asks a small local model to read up to a few files, or to condense one text, and answer one task with cited \
         evidence. Its answer is a reading, not a verified fact: check the cited lines before you rely on it. It cannot \
         edit files or run commands."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "role": {"type": "string", "enum": ["explore", "summarise"]},
                "task": {"type": "string", "maxLength": 500},
                "paths": {"type": "array", "items": {"type": "string"}, "maxItems": 8},
                "text": {"type": "string", "maxLength": 8000}
            },
            "required": ["role", "task"],
            "additionalProperties": false
        })
    }

    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let role = args
            .get("role")
            .and_then(Value::as_str)
            .and_then(Role::parse)
            .ok_or_else(|| "role must be \"explore\" or \"summarise\"".to_owned())?;
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| "task must be a string".to_owned())?
            .to_owned();
        let paths = match args.get("paths") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "paths must be strings".to_owned())
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("paths must be an array of strings".to_owned()),
        };
        let text = match args.get("text") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err("text must be a string".to_owned()),
        };
        let request = Request {
            role,
            task,
            paths,
            text,
        };
        let outcome = run_for_config(
            &request,
            &ctx.config,
            &ctx.workspace,
            &ctx.policy,
            &ctx.redactor,
            &|name| std::env::var(name).ok(),
        )?;
        Ok(ToolOutput {
            text: ctx.redactor.redact(&outcome.rendered).into_owned(),
            structured: Some(json!({
                "model": outcome.model,
                "latency_ms": outcome.latency_ms,
                "inputs_left_out": outcome.inputs_left_out,
                "answer": outcome.answer,
            })),
            is_error: false,
        })
    }
}
