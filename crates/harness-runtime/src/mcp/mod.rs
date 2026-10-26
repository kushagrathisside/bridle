//! A minimal Model Context Protocol server over stdio (ADR-0001 §2).
//!
//! Only the surface the harness needs: `initialize`, `notifications/initialized`, `ping`,
//! `tools/list` and `tools/call`. Messages are newline-delimited JSON-RPC 2.0. Any MCP-capable
//! host (OpenCode today) can load these tools, which keeps them independent of the host.

pub mod protocol;
pub mod record;
pub mod tools;

pub use protocol::{PROTOCOL_VERSIONS, Server, ServerInfo};
pub use tools::adapters::adapter_tools;
pub use tools::{McpTool, ToolContext, ToolOutput, default_tools};
