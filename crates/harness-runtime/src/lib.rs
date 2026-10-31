//! `harness-runtime`: everything that talks to the outside world on the harness's behalf.
//!
//! Model providers speak HTTP to local inference servers; the launcher starts those servers; the
//! event log appends telemetry; the MCP server exposes deterministic tools to the coding-agent
//! host. Policy decisions are never made here — they come from `harness-core`.

pub mod artifacts;
pub mod code;
pub mod decision;
pub mod event_log;
pub mod health;
pub mod http;
pub mod jev;
pub mod launcher;
pub mod laya;
pub mod manifest;
pub mod mcp;
pub mod memory;
pub mod providers;
pub mod routing;
pub mod sandbox;
pub mod state;
pub mod subagent;
pub mod sysprobe;
