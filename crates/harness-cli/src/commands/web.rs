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
    /// The managed browser (Chromium through Playwright): status, install, remove (ADR-0020).
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum BrowserAction {
    /// Show whether the browser is enabled, installed and what versions it has.
    Status,
    /// Download and install the pinned runtime (Node.js, playwright-core, Chromium) into the managed folder.
    Setup,
    /// Start the browser against a small local web site and check what it can and cannot do. Needs no internet.
    Test {
        /// Also open one real page on the real network, through your proxy: an optional live smoke check.
        #[arg(long)]
        live: Option<String>,
    },
    /// Remove the managed runtime folder.
    Remove,
}

fn run_browser(app: &App, action: &BrowserAction) -> Result<ExitCode> {
    use harness_runtime::web::browser::runtime;
    let Some(layout) = runtime::Layout::default_location() else {
        bail!("cannot tell where the machine configuration lives (set BRIDLE_HOME)");
    };
    let env = |name: &str| std::env::var(name).ok();
    match action {
        BrowserAction::Status => {
            let b = &app.config.web.browser;
            println!(
                "browser: {}",
                if b.enabled {
                    "ON"
                } else {
                    "off (set [web.browser] enabled = true in the machine configuration)"
                }
            );
            println!("runtime folder: {}", layout.root.display());
            let status = runtime::inspect(&layout);
            println!("runtime: {}", status.describe());
            if let Some(m) = &status.manifest {
                println!(
                    "  Node.js {}, playwright-core {}, Chromium {} (revision {}), worker protocol {}",
                    m.node_version,
                    m.playwright_version,
                    m.chromium_version,
                    m.chromium_revision,
                    m.protocol
                );
                println!("  installed {} for {}", m.installed_at, m.platform);
                println!("  chrome.exe SHA-256 {}", m.chromium_executable_sha256);
            }
            println!(
                "limits: {} session(s), navigation {} ms, action {} ms, idle {} s, page text {} characters; screenshots {}, downloads {}",
                b.max_sessions,
                b.navigation_timeout_ms,
                b.action_timeout_ms,
                b.session_idle_secs,
                b.max_page_content_chars,
                if b.screenshots_enabled { "on" } else { "off" },
                if b.downloads_enabled { "on" } else { "off" }
            );
            println!(
                "model-facing tools (browser_navigate, browser_snapshot, browser_action): {}",
                match app.config.security.network {
                    harness_core::config::Verdict::Allow => "allowed (security.network = allow)",
                    harness_core::config::Verdict::Ask =>
                        "blocked: security.network is `ask`, which the harness never allows",
                    harness_core::config::Verdict::Deny => "denied (security.network = deny)",
                }
            );
            let route = http::remote_route();
            println!("route: {}", route.describe());
            match harness_runtime::web::browser::proxy::Upstream::from_route(&route, &env) {
                Ok(upstream) => println!("egress: {}", upstream.describe()),
                Err(error) => {
                    println!("egress: the browser cannot start with this proxy setting: {error}")
                }
            }
            println!(
                "pinned by this Bridle: Node.js {}, playwright-core {}, Chromium {} (revision {})",
                runtime::NODE_VERSION,
                runtime::PLAYWRIGHT_VERSION,
                runtime::CHROMIUM_VERSION,
                runtime::CHROMIUM_REVISION
            );
            Ok(if status.ready() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        BrowserAction::Setup => {
            let route = http::remote_route();
            println!("Route: {}", route.describe());
            let mut say = |line: &str| println!("{line}");
            match runtime::setup(&layout, &route, &env, &mut say) {
                Ok(m) => {
                    println!(
                        "Installed: Node.js {}, playwright-core {}, Chromium {} in {}",
                        m.node_version,
                        m.playwright_version,
                        m.chromium_version,
                        layout.root.display()
                    );
                    println!("chrome.exe SHA-256 {}", m.chromium_executable_sha256);
                    Ok(ExitCode::SUCCESS)
                }
                Err(error) => {
                    eprintln!("bridle: browser setup failed: {error}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        BrowserAction::Test { live } => {
            let status = runtime::inspect(&layout);
            if !status.ready() {
                eprintln!("bridle: the browser runtime is {}", status.describe());
                return Ok(ExitCode::FAILURE);
            }
            let route = http::remote_route();
            let mut say = |line: &str| println!("{line}");
            println!(
                "Running the browser self-test (a real Chromium against a site served from this machine) ..."
            );
            match harness_runtime::web::browser::selftest::run(&layout, &route, &env, &mut say) {
                Ok(checks) => {
                    let failed = checks.iter().filter(|c| !c.ok).count();
                    for c in &checks {
                        println!(
                            "{} {} - {}",
                            if c.ok { "PASS" } else { "FAIL" },
                            c.name,
                            c.detail
                        );
                    }
                    println!(
                        "{} check(s), {} passed, {} failed",
                        checks.len(),
                        checks.len() - failed,
                        failed
                    );
                    let mut live_failed = 0;
                    if let Some(url) = live {
                        println!();
                        println!(
                            "LIVE smoke check (real network, not part of the deterministic test): {url}"
                        );
                        match harness_runtime::web::browser::selftest::live(
                            &layout, &route, &env, url, &mut say,
                        ) {
                            Ok(live_checks) => {
                                for c in &live_checks {
                                    println!(
                                        "{} {} - {}",
                                        if c.ok { "PASS" } else { "FAIL" },
                                        c.name,
                                        c.detail
                                    );
                                }
                                live_failed = live_checks.iter().filter(|c| !c.ok).count();
                            }
                            Err(error) => {
                                eprintln!("bridle: the live check could not run: {error}");
                                live_failed = 1;
                            }
                        }
                    }
                    Ok(if failed + live_failed == 0 {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    })
                }
                Err(error) => {
                    eprintln!("bridle: browser self-test could not run: {error}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        BrowserAction::Remove => {
            match runtime::remove(&layout) {
                Ok(true) => println!("Removed {}", layout.root.display()),
                Ok(false) => println!(
                    "Nothing to remove: {} does not exist",
                    layout.root.display()
                ),
                Err(error) => {
                    eprintln!("bridle: {error}");
                    return Ok(ExitCode::FAILURE);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
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
        WebAction::Browser { action } => run_browser(app, action),
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
