//! JSON-RPC 2.0 / MCP message handling.
//!
//! IMPLEMENTATION SPEC (delegated):
//! - `Server::new(info, tools, ctx)`; `Server::serve(&mut self, reader: impl BufRead, writer:
//!   impl Write) -> io::Result<()>` loops over lines until EOF. Blank lines are ignored. Each line
//!   is handled by the pure `handle_line(&mut self, line: &str) -> Option<String>` (returns the
//!   response line without trailing newline, `None` for notifications), which `serve` writes
//!   followed by `\n` and a flush. Log nothing to stdout except protocol messages (stdout is the
//!   transport); diagnostics go to stderr.
//! - Parse errors → `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}`.
//!   A message without `"jsonrpc":"2.0"` or without `method` (and not a response) → -32600.
//!   Messages without `id` are notifications: never answer them (including unknown ones).
//!   JSON-RPC batches (arrays) → -32600 "batches not supported".
//! - `initialize`: `params.protocolVersion` — if it is in [`PROTOCOL_VERSIONS`] echo it, else
//!   answer with `PROTOCOL_VERSIONS[0]` (the newest). Result: `{"protocolVersion", "capabilities":
//!   {"tools": {"listChanged": false}}, "serverInfo": {"name", "version"}, "instructions"}`.
//! - `ping` → `{}`.
//! - `tools/list` → `{"tools": [{"name", "description", "inputSchema"}...]}` in registration order.
//! - `tools/call` → `params.name` unknown → JSON-RPC error -32602 "unknown tool: X". Missing or
//!   non-object `arguments` → treat as `{}`. Otherwise call the tool; `ToolOutput { text,
//!   structured, is_error }` maps to `{"content":[{"type":"text","text"}], "isError": is_error}`
//!   plus `"structuredContent": structured` when present. A tool returning `Err(msg)` maps to a
//!   *successful* JSON-RPC response with `isError: true` and the message as text (MCP convention:
//!   tool failures are results, not protocol errors). Catch panics from tools with
//!   `std::panic::catch_unwind(AssertUnwindSafe(..))` → `isError: true`, "internal tool error".
//! - Any other method → -32601 "Method not found".
//! - Echo the request `id` exactly (number or string).
//! - Tests in `crates/harness-runtime/tests/mcp_protocol.rs`: full handshake via `serve` over
//!   in-memory buffers, version negotiation (supported, unsupported), notifications produce no
//!   output, parse error, unknown method, unknown tool, tool error → isError, string ids.

use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};

use harness_core::security::redact::Redactor;

use super::tools::{McpTool, ToolContext, ToolOutput};

/// Supported protocol revisions, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Static identity of the server, reported in the `initialize` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// Server name, e.g. `harness`.
    pub name: String,
    /// Server version string.
    pub version: String,
    /// Free-text instructions returned to the client at initialisation.
    pub instructions: String,
}

/// A stdio MCP server: the tools it offers plus the context they run in.
pub struct Server {
    /// Identity reported to clients.
    pub info: ServerInfo,
    /// Tools in registration order; `tools/list` reports them in this order.
    pub tools: Vec<Box<dyn McpTool>>,
    /// Shared context handed to every tool call.
    pub ctx: ToolContext,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("info", &self.info)
            .field("tools", &self.tools.len())
            .finish()
    }
}

/// A JSON-RPC protocol error, sent as the `error` member of a response.
#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Server {
    /// Creates a server that offers `tools` and runs them in `ctx`.
    pub fn new(info: ServerInfo, tools: Vec<Box<dyn McpTool>>, ctx: ToolContext) -> Self {
        Self { info, tools, ctx }
    }

    /// Reads newline-delimited requests from `reader` until EOF and writes one response line per
    /// answered request to `writer`. Blank lines are skipped and notifications produce no output.
    pub fn serve(
        &mut self,
        mut reader: impl BufRead,
        mut writer: impl Write,
    ) -> std::io::Result<()> {
        let mut raw = Vec::new();
        loop {
            raw.clear();
            if reader.read_until(b'\n', &mut raw)? == 0 {
                return Ok(());
            }
            let response = match std::str::from_utf8(&raw) {
                Ok(line) => self.handle_line(line),
                Err(_) => {
                    Some(error_response(&Value::Null, PARSE_ERROR, "Parse error").to_string())
                }
            };
            if let Some(response) = response {
                writer.write_all(response.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }

    /// Handles one request line and returns the response line (without a trailing newline), or
    /// `None` when the line is blank, a notification, or a client response.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(message) => self.dispatch(&message).map(|response| response.to_string()),
            Err(_) => Some(error_response(&Value::Null, PARSE_ERROR, "Parse error").to_string()),
        }
    }

    /// Validates the envelope of one parsed message and routes it. Returns the response value,
    /// or `None` when nothing should be sent.
    fn dispatch(&self, message: &Value) -> Option<Value> {
        if message.is_array() {
            return Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "batches not supported",
            ));
        }
        let Some(object) = message.as_object() else {
            return Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "Invalid Request",
            ));
        };

        let id = object.get("id");
        let reply_id = match id {
            None => Value::Null,
            Some(value) if value.is_string() || value.is_number() || value.is_null() => {
                value.clone()
            }
            Some(_) => {
                return Some(error_response(
                    &Value::Null,
                    INVALID_REQUEST,
                    "Invalid Request: id must be a string, number or null",
                ));
            }
        };

        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                &reply_id,
                INVALID_REQUEST,
                "Invalid Request: jsonrpc must be \"2.0\"",
            ));
        }

        let Some(method_value) = object.get("method") else {
            // A response to a request this server never sent: nothing to answer.
            if object.contains_key("result") || object.contains_key("error") {
                return None;
            }
            return Some(error_response(
                &reply_id,
                INVALID_REQUEST,
                "Invalid Request: missing method",
            ));
        };
        let Some(method) = method_value.as_str() else {
            return Some(error_response(
                &reply_id,
                INVALID_REQUEST,
                "Invalid Request: method must be a string",
            ));
        };

        if method == "notifications/initialized" {
            return None;
        }
        let outcome = self.route(method, object.get("params"));
        // Notifications (no id) are never answered, whatever their outcome.
        id?;
        Some(match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": reply_id, "result": result}),
            Err(error) => error_response(&reply_id, error.code, &error.message),
        })
    }

    /// Dispatches a request by method name.
    fn route(&self, method: &str, params: Option<&Value>) -> Result<Value, RpcError> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => self.call_tool(params),
            _ => Err(RpcError::new(METHOD_NOT_FOUND, "Method not found")),
        }
    }

    /// Answers `initialize`, echoing the client's protocol version when it is supported.
    fn initialize(&self, params: Option<&Value>) -> Value {
        let requested = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);
        let version = match requested {
            Some(version) if PROTOCOL_VERSIONS.contains(&version) => version,
            _ => PROTOCOL_VERSIONS.first().copied().unwrap_or_default(),
        };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": self.info.name, "version": self.info.version},
            "instructions": self.info.instructions,
        })
    }

    /// Answers `tools/list` with every registered tool, in registration order.
    fn list_tools(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "inputSchema": tool.input_schema(),
                })
            })
            .collect();
        json!({"tools": tools})
    }

    /// Answers `tools/call`. Unknown or unnamed tools are protocol errors; everything a tool does
    /// after it is found is reported inside a successful result.
    fn call_tool(&self, params: Option<&Value>) -> Result<Value, RpcError> {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "missing tool name"))?;
        let Some(tool) = self.tools.iter().find(|tool| tool.name() == name) else {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool: {name}"),
            ));
        };
        let args = params
            .and_then(|p| p.get("arguments"))
            .filter(|arguments| arguments.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let output = run_tool(tool.as_ref(), &args, &self.ctx);
        // Every result leaves the server redacted, whatever the tool returned. The host's
        // after-hook may not see MCP output (observed 2026-10-21), so this is the guarantee.
        let output = redact_output(output, &self.ctx.redactor);
        Ok(tool_result(output))
    }
}

/// Runs one tool, turning a panic into an error output so the server keeps serving.
fn run_tool(tool: &dyn McpTool, args: &Value, ctx: &ToolContext) -> ToolOutput {
    match catch_unwind(AssertUnwindSafe(|| tool.call(args, ctx))) {
        Ok(Ok(output)) => output,
        Ok(Err(message)) => tool_error(message),
        Err(_) => {
            eprintln!("harness mcp: tool `{}` panicked", tool.name());
            tool_error("internal tool error")
        }
    }
}

/// Redacts a tool output's text and structured content with the server's redactor.
fn redact_output(output: ToolOutput, redactor: &Redactor) -> ToolOutput {
    let ToolOutput {
        text,
        mut structured,
        is_error,
    } = output;
    if let Some(value) = structured.as_mut() {
        redactor.redact_json(value);
    }
    ToolOutput {
        text: redactor.redact(&text).into_owned(),
        structured,
        is_error,
    }
}

fn tool_error(message: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: message.into(),
        structured: None,
        is_error: true,
    }
}

/// Maps a tool output to the MCP `CallToolResult` shape.
fn tool_result(output: ToolOutput) -> Value {
    let ToolOutput {
        text,
        structured,
        is_error,
    } = output;
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    result
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::Cursor;

    use serde_json::{Value, json};

    use super::*;
    use crate::mcp::tools::{default_tools, test_context};

    /// Returns its arguments as both text and structured content.
    struct Echo;

    impl McpTool for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "Echoes its arguments."
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        fn call(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput, String> {
            Ok(ToolOutput {
                text: args.to_string(),
                structured: Some(args.clone()),
                is_error: false,
            })
        }
    }

    /// Always fails with a tool-level error.
    struct Fail;

    impl McpTool for Fail {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn description(&self) -> &'static str {
            "Always fails."
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        fn call(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput, String> {
            Err("boom".to_owned())
        }
    }

    /// Panics inside the call.
    struct Explode;

    impl McpTool for Explode {
        fn name(&self) -> &'static str {
            "explode"
        }
        fn description(&self) -> &'static str {
            "Panics."
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        fn call(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput, String> {
            panic!("tool exploded")
        }
    }

    fn test_server() -> Server {
        let workspace = tempfile::tempdir().unwrap();
        let ctx = test_context(workspace.path());
        let tools: Vec<Box<dyn McpTool>> = vec![Box::new(Echo), Box::new(Fail), Box::new(Explode)];
        Server::new(
            ServerInfo {
                name: "harness".to_owned(),
                version: "0.0.0-test".to_owned(),
                instructions: "test server".to_owned(),
            },
            tools,
            ctx,
        )
    }

    /// Feeds `lines` through `serve` and parses every response line.
    fn exchange(lines: &[&str]) -> Vec<Value> {
        let mut server = test_server();
        let input: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut output = Vec::new();
        server
            .serve(Cursor::new(input.into_bytes()), &mut output)
            .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn one(line: &str) -> Value {
        let mut responses = exchange(&[line]);
        assert_eq!(responses.len(), 1, "expected exactly one response");
        responses.remove(0)
    }

    #[test]
    fn handshake_over_buffers() {
        let responses = exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}"#,
        ]);
        assert_eq!(responses.len(), 4, "the notification must not be answered");

        let init = &responses[0];
        assert_eq!(init["id"], json!(1));
        assert_eq!(init["result"]["protocolVersion"], json!("2025-06-18"));
        assert_eq!(
            init["result"]["capabilities"]["tools"]["listChanged"],
            json!(false)
        );
        assert_eq!(init["result"]["serverInfo"]["name"], json!("harness"));
        assert_eq!(init["result"]["instructions"], json!("test server"));

        assert_eq!(
            responses[1],
            json!({"jsonrpc": "2.0", "id": 2, "result": {}})
        );

        let listed = responses[2]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = listed.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["echo", "fail", "explode"]);
        assert!(listed[0]["inputSchema"].is_object());

        let call = &responses[3]["result"];
        assert_eq!(call["isError"], json!(false));
        assert_eq!(call["structuredContent"], json!({"x": 1}));
        assert_eq!(call["content"][0]["type"], json!("text"));
    }

    #[test]
    fn supported_version_is_echoed() {
        let response = one(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        assert_eq!(response["result"]["protocolVersion"], json!("2024-11-05"));
    }

    #[test]
    fn unsupported_version_gets_newest_supported() {
        let response = one(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(
            response["result"]["protocolVersion"],
            json!(PROTOCOL_VERSIONS[0])
        );
    }

    #[test]
    fn missing_version_gets_newest_supported() {
        let response = one(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert_eq!(
            response["result"]["protocolVersion"],
            json!(PROTOCOL_VERSIONS[0])
        );
    }

    #[test]
    fn notifications_are_never_answered() {
        let responses = exchange(&[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            r#"{"jsonrpc":"2.0","method":"no/such/method"}"#,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"nope"}}"#,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"fail"}}"#,
        ]);
        assert!(responses.is_empty(), "got {responses:?}");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let responses = exchange(&["", "   ", r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#]);
        assert_eq!(responses.len(), 1);
    }

    #[test]
    fn parse_error_has_null_id() {
        let response = one("{not json");
        assert_eq!(
            response,
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}})
        );
    }

    #[test]
    fn invalid_utf8_is_a_parse_error() {
        let mut server = test_server();
        let mut output = Vec::new();
        server
            .serve(Cursor::new(vec![0xff, 0xfe, b'\n']), &mut output)
            .unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], json!(-32700));
    }

    #[test]
    fn invalid_requests_get_minus_32600() {
        let wrong_version = one(r#"{"jsonrpc":"1.0","id":5,"method":"ping"}"#);
        assert_eq!(wrong_version["id"], json!(5));
        assert_eq!(wrong_version["error"]["code"], json!(-32600));

        let no_version = one(r#"{"id":6,"method":"ping"}"#);
        assert_eq!(no_version["error"]["code"], json!(-32600));

        let no_method = one(r#"{"jsonrpc":"2.0","id":7}"#);
        assert_eq!(no_method["id"], json!(7));
        assert_eq!(no_method["error"]["code"], json!(-32600));

        let batch = one(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
        assert_eq!(batch["id"], Value::Null);
        assert_eq!(batch["error"]["message"], json!("batches not supported"));

        let scalar = one("42");
        assert_eq!(scalar["error"]["code"], json!(-32600));
    }

    #[test]
    fn unknown_method_gets_minus_32601_with_id() {
        let response = one(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#);
        assert_eq!(response["id"], json!(7));
        assert_eq!(response["error"]["code"], json!(-32601));
        assert_eq!(response["error"]["message"], json!("Method not found"));
    }

    #[test]
    fn unknown_tool_gets_minus_32602() {
        let response = one(
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        );
        assert_eq!(response["id"], json!(8));
        assert_eq!(response["error"]["code"], json!(-32602));
        assert_eq!(response["error"]["message"], json!("unknown tool: nope"));
    }

    #[test]
    fn tool_error_is_a_successful_result_with_is_error() {
        let response = one(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"fail","arguments":{}}}"#,
        );
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], json!(true));
        assert_eq!(response["result"]["content"][0]["text"], json!("boom"));
        assert!(response["result"].get("structuredContent").is_none());
    }

    #[test]
    fn panicking_tool_is_contained() {
        let response =
            one(r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"explode"}}"#);
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], json!(true));
        assert_eq!(
            response["result"]["content"][0]["text"],
            json!("internal tool error")
        );
    }

    #[test]
    fn missing_arguments_are_treated_as_empty_object() {
        let response = one(
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"echo","arguments":"oops"}}"#,
        );
        assert_eq!(response["result"]["structuredContent"], json!({}));
    }

    #[test]
    fn string_ids_are_echoed_exactly() {
        let response = one(r#"{"jsonrpc":"2.0","id":"req-1","method":"ping"}"#);
        assert_eq!(response["id"], json!("req-1"));
        let response = one(r#"{"jsonrpc":"2.0","id":"abc","method":"unknown"}"#);
        assert_eq!(response["id"], json!("abc"));
        assert_eq!(response["error"]["code"], json!(-32601));
    }

    #[test]
    fn numeric_and_null_ids_are_echoed() {
        let response = one(r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#);
        assert_eq!(response["id"], json!(42));
        let response = one(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#);
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["result"], json!({}));
    }

    #[test]
    fn client_responses_are_ignored() {
        assert_eq!(
            test_server().handle_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            None
        );
    }

    #[test]
    fn tools_list_reports_default_tools_in_order() {
        let mut server = test_server();
        server.tools = default_tools();
        let response = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap();
        let value: Value = serde_json::from_str(&response).unwrap();
        let names: Vec<&str> = value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["git_status", "git_diff", "run_verification", "harness_info"]
        );
    }

    #[test]
    fn tool_results_are_redacted_before_they_leave_the_server() {
        let output = ToolOutput {
            text: "API_KEY=sk-test-SECRET123456789012".to_owned(),
            structured: Some(json!({"password": "hunter2", "note": "ok"})),
            is_error: false,
        };
        let redacted = redact_output(output, &Redactor::new());
        assert!(
            !redacted.text.contains("SECRET123456789012"),
            "{}",
            redacted.text
        );
        let structured = redacted.structured.expect("structured kept");
        assert_eq!(structured["note"], json!("ok"));
        assert!(!structured.to_string().contains("hunter2"), "{structured}");
    }
}
