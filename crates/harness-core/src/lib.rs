//! `harness-core`: the deterministic heart of the harness.
//!
//! Everything in this crate is pure logic over data: configuration, the capability-based model
//! registry, the security policy (path guard, secret redaction, command risk, tool policy) and the
//! telemetry event schema. It performs read-only filesystem access (path canonicalisation, config
//! files) but never touches the network, never spawns processes and knows nothing about OpenCode.
//! Host-specific code lives in `harness-cli::host`.

pub mod config;
pub mod context;
pub mod decision;
pub mod failures;
pub mod fingerprint;
pub mod identity;
pub mod jev;
pub mod journal;
pub mod lfm;
pub mod loops;
pub mod memory;
pub mod models;
pub mod routing;
pub mod security;
pub mod subagent;
pub mod task;
pub mod team;
pub mod telemetry;

pub use config::Config;
