//! `bridle web`: the web research tools for a person at the terminal (ADR-0019). The person is the actor here, so the commands do
//! not go through the model-facing policy gate (`security.network`); they need `[web] enabled` in the machine configuration and,
//! for a search, a usable provider. Nothing here prints an API key: only the NAME of the variable that holds it, and whether it is set.

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Subcommand;
use harness_core::config::{WebProviderKind, endpoint_is_loopback};
use harness_runtime::http;
use harness_runtime::mcp::tools::web::{inspect_file, run_download, run_open, run_search};
use harness_runtime::web::fetch::Fetcher;
use harness_runtime::web::providers::{self, Kind, SearchRequest};
use harness_runtime::web::store::Sources;

use crate::app::App;

#[derive(Debug, Subcommand)]
pub enum WebAction {
    /// List the configured search providers and whether each can be used.
    Providers,
    /// Show the web research settings, the route requests take, and what has been recorded.
    Status,
    /// Search with every usable provider and print the fused results.
    Search {
        query: String,
        /// Search the news instead of the web.
        #[arg(long)]
        news: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Only these sites (repeat the option).
        #[arg(long = "domain")]
        domains: Vec<String>,
        /// Only results published on or after this date, YYYY-MM-DD.
        #[arg(long)]
        after: Option<String>,
    },
    /// Open a page and print its text.
    Open { url: String },
    /// Download a file into the project's web downloads folder.
    Download {
        url: String,
        /// The expected SHA-256.
        #[arg(long)]
        sha256: Option<String>,
    },
    /// Show a downloaded file (by source id, such as S-0003) or a file in the project.
    Inspect { target: String },
}

pub fn run(app: &App, action: &WebAction) -> Result<ExitCode> {
    let web = &app.config.web;
    let env = |name: &str| std::env::var(name).ok();
    let route = http::remote_route();
    match action {
        WebAction::Providers => {
            if web.providers.is_empty() {
                println!(
                    "No provider is configured. Add one under [web.providers.<name>] in the machine configuration."
                );
            }
            for (name, p) in &web.providers {
                let kind = match p.kind {
                    WebProviderKind::Searxng => "searxng",
                    WebProviderKind::Brave => "brave",
                };
                let key = match (&p.kind, p.api_key_env.as_deref()) {
                    (WebProviderKind::Brave, Some(var)) => format!(
                        "key in ${var}: {}",
                        if env(var).is_some_and(|v| !v.trim().is_empty()) {
                            "set"
                        } else {
                            "NOT SET"
                        }
                    ),
                    _ => "no key".to_owned(),
                };
                let url = p
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "(default address)".to_owned());
                println!(
                    "{name}: {kind}, {}, {url}{}, {key}",
                    if p.enabled { "enabled" } else { "disabled" },
                    if endpoint_is_loopback(&url) {
                        " (this machine)"
                    } else {
                        ""
                    }
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        WebAction::Status => {
            println!(
                "web research: {}",
                if web.enabled {
                    "ON"
                } else {
                    "off (set [web] enabled = true in the machine configuration)"
                }
            );
            println!(
                "model-facing tools (web_search, web_open, web_download): {}",
                match app.config.security.network {
                    harness_core::config::Verdict::Allow => "allowed (security.network = allow)",
                    harness_core::config::Verdict::Ask =>
                        "blocked: security.network is `ask`, which the harness never allows",
                    harness_core::config::Verdict::Deny => "denied (security.network = deny)",
                }
            );
            println!("route: {}", route.describe());
            println!(
                "limits: {} results, {} ms timeout, pages up to {} bytes, downloads up to {} MiB",
                web.max_results, web.timeout_ms, web.max_fetch_bytes, web.max_download_mb
            );
            let sources = Sources::open(&app.workspace);
            println!(
                "recorded sources: {} (in .bridle/web/)",
                sources.list().len()
            );
            let (usable, skipped) = providers::build(web, &route, &env);
            println!(
                "usable providers: {}",
                if usable.is_empty() {
                    "none".to_owned()
                } else {
                    usable
                        .iter()
                        .map(|p| p.name().to_owned())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            for s in skipped {
                println!("  not usable: {s}");
            }
            Ok(ExitCode::SUCCESS)
        }
        WebAction::Search {
            query,
            news,
            limit,
            domains,
            after,
        } => {
            need_enabled(app)?;
            let (usable, skipped) = providers::build(web, &route, &env);
            if usable.is_empty() {
                bail!(
                    "no search provider is usable: {}",
                    if skipped.is_empty() {
                        "none is enabled".to_owned()
                    } else {
                        skipped.join("; ")
                    }
                );
            }
            let request = SearchRequest {
                query: query.clone(),
                kind: if *news { Kind::News } else { Kind::Web },
                limit: (*limit).clamp(1, web.max_results),
                domains: domains.clone(),
                after: after.clone(),
            };
            print!(
                "{}",
                run_search(&usable, &Sources::open(&app.workspace), &request, 2)
                    .map_err(anyhow::Error::msg)?
            );
            Ok(ExitCode::SUCCESS)
        }
        WebAction::Open { url } => {
            need_enabled(app)?;
            print!(
                "{}",
                run_open(
                    &fetcher(app, &route),
                    &Sources::open(&app.workspace),
                    url,
                    12_000
                )
                .map_err(anyhow::Error::msg)?
            );
            Ok(ExitCode::SUCCESS)
        }
        WebAction::Download { url, sha256 } => {
            need_enabled(app)?;
            let max = web.max_download_mb.saturating_mul(1024 * 1024);
            print!(
                "{}",
                run_download(
                    &fetcher(app, &route),
                    &Sources::open(&app.workspace),
                    url,
                    max,
                    sha256.as_deref()
                )
                .map_err(anyhow::Error::msg)?
            );
            Ok(ExitCode::SUCCESS)
        }
        WebAction::Inspect { target } => {
            let sources = Sources::open(&app.workspace);
            if target.starts_with("S-") {
                let record = sources
                    .list()
                    .into_iter()
                    .find(|r| r.id == *target && r.kind == "download")
                    .ok_or_else(|| anyhow::anyhow!("there is no downloaded source {target}"))?;
                let found = std::fs::read_dir(sources.downloads_dir())?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| {
                        p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with(&record.sha256[..8]))
                    });
                let path = found.ok_or_else(|| {
                    anyhow::anyhow!("the file of {target} is no longer in the downloads folder")
                })?;
                print!(
                    "{}",
                    inspect_file(&path, target, 12_000).map_err(anyhow::Error::msg)?
                );
            } else {
                let path = app.workspace.join(target);
                print!(
                    "{}",
                    inspect_file(&path, target, 12_000).map_err(anyhow::Error::msg)?
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn need_enabled(app: &App) -> Result<()> {
    if !app.config.web.enabled {
        bail!(
            "web research is off: set [web] enabled = true and a provider in the machine configuration (see `bridle web status`)"
        );
    }
    Ok(())
}

fn fetcher(app: &App, route: &http::RemoteRoute) -> Fetcher {
    Fetcher::new(
        route.clone(),
        Duration::from_millis(app.config.web.timeout_ms),
        app.config.web.max_fetch_bytes,
        &|n| std::env::var(n).ok(),
    )
}
