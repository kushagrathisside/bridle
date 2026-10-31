//! Configured adapters: a command declared in `[[tools.adapters]]`
//! becomes an MCP tool named `adapter_<name>`. The harness core hard-codes no project. Every
//! call goes through `run::execute_plain`, so it gets the same checks as `run_command`.

use harness_core::config::{AdapterConfig, Config};
use serde_json::{Value, json};

use super::run::{execute_plain, plain_word};
use super::{McpTool, ToolContext, ToolOutput, optional_string_list};

/// One configured adapter as a tool.
#[derive(Debug)]
pub struct AdapterTool {
    name: &'static str,
    description: &'static str,
    config: AdapterConfig,
}

impl AdapterTool {
    fn new(config: &AdapterConfig) -> Self {
        // Adapter names are fixed for the life of the process, so the strings are created once
        // per configured adapter and kept for the tool's lifetime.
        Self {
            name: Box::leak(format!("adapter_{}", config.name).into_boxed_str()),
            description: Box::leak(config.description.clone().into_boxed_str()),
            config: config.clone(),
        }
    }
}

impl McpTool for AdapterTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn input_schema(&self) -> Value {
        if self.config.accepts_args {
            json!({
                "type": "object",
                "properties": {"args": {"type": "array", "items": {"type": "string"},
                    "description": "Extra plain arguments appended after the adapter's fixed ones."}},
                "additionalProperties": false,
            })
        } else {
            json!({"type": "object", "properties": {}, "additionalProperties": false})
        }
    }
    fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let extra = optional_string_list(args, "args")?.unwrap_or_default();
        if !extra.is_empty() && !self.config.accepts_args {
            return Err(format!("`{}` takes no arguments", self.name));
        }
        if let Some(bad) = extra.iter().find(|word| !plain_word(word)) {
            return Err(format!(
                "argument `{bad}` uses characters adapters do not pass"
            ));
        }
        let mut words = self.config.args.clone();
        words.extend(extra);
        let timeout = self
            .config
            .timeout_ms
            .unwrap_or(ctx.verification.default_timeout_ms)
            .clamp(1_000, 600_000);
        execute_plain(ctx, &self.config.program, &words, None, timeout)
    }
}

/// The adapter tools for a configuration, in declaration order.
pub fn adapter_tools(config: &Config) -> Vec<Box<dyn McpTool>> {
    config
        .tools
        .adapters
        .iter()
        .map(|adapter| Box::new(AdapterTool::new(adapter)) as Box<dyn McpTool>)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::test_context;
    use super::*;

    fn adapter(name: &str, program: &str, args: &[&str], accepts: bool) -> AdapterConfig {
        AdapterConfig {
            name: name.to_owned(),
            description: format!("test adapter {name}"),
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            accepts_args: accepts,
            timeout_ms: None,
        }
    }

    #[test]
    fn adapters_become_named_tools_in_order() {
        let mut config = Config::default();
        config.tools.adapters = vec![
            adapter("status", "git", &["status"], false),
            adapter("log", "git", &["log"], true),
        ];
        let tools = adapter_tools(&config);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["adapter_status", "adapter_log"]);
        assert_eq!(tools[0].description(), "test adapter status");
    }

    #[test]
    fn an_adapter_runs_its_fixed_command_and_checks_extra_words() {
        let Some(dir) = super::super::test_support::init_repo() else {
            return;
        };
        let ctx = test_context(dir.path());
        let fixed = AdapterTool::new(&adapter("status", "git", &["status", "--short"], false));
        let out = fixed.call(&json!({}), &ctx).unwrap().structured.unwrap();
        assert_eq!(out["status"], json!("passed"), "{out}");
        assert!(fixed.call(&json!({"args": ["-x"]}), &ctx).is_err());

        let with_args = AdapterTool::new(&adapter("status2", "git", &["status"], true));
        assert!(with_args.call(&json!({"args": ["--short"]}), &ctx).is_ok());
        assert!(with_args.call(&json!({"args": ["x;y"]}), &ctx).is_err());
    }

    #[test]
    fn an_adapter_cannot_run_a_destructive_command() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_context(dir.path());
        let tool = AdapterTool::new(&adapter("wipe", "git", &["clean", "-fdx"], false));
        let out = tool.call(&json!({}), &ctx).unwrap().structured.unwrap();
        assert_eq!(out["status"], json!("blocked"), "{out}");
    }
}
