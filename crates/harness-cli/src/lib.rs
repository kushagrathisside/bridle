//! The harness application as a library: the commands, the agent loop behind the chat, the
//! orchestrator workflow and the terminal UI. The `bridle-cli` binary (`main.rs`) is argument parsing on
//! top of this crate, and the desktop app (`bridle-desktop`) is a second front end to the same code.
//! Neither is a privileged host: both go through the same policy, loop rules and event log.

pub mod app;
pub mod board;
pub mod board_tools;
pub mod chat;
pub mod cli;
pub mod commands;
pub mod crew;
pub mod gitlayer;
pub mod host;
pub mod ideas;
pub mod image;
pub mod output;
pub mod prompt;
pub mod route;
pub mod tui;
pub mod workflow;
