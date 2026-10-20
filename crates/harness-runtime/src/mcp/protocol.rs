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

use super::tools::{McpTool, ToolContext};

/// Supported protocol revisions, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub instructions: String,
}

pub struct Server {
    pub info: ServerInfo,
    pub tools: Vec<Box<dyn McpTool>>,
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

impl Server {
    pub fn new(info: ServerInfo, tools: Vec<Box<dyn McpTool>>, ctx: ToolContext) -> Self {
        Self { info, tools, ctx }
    }

    pub fn serve(&mut self, reader: impl BufRead, writer: impl Write) -> std::io::Result<()> {
        let _ = (reader, writer);
        todo!()
    }

    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let _ = line;
        todo!()
    }
}
