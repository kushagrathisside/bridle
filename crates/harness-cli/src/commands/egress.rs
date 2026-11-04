//! `bridle egress`: every outbound destination this configuration can reach.
//!
//! Read-only. It reads the config and the names of environment variables. It sends nothing, and it never prints a proxy
//! address or a key. The list is the audit's first step: the security page says what each destination carries.

use std::process::ExitCode;

use anyhow::Result;
use harness_core::config::{Config, DecisionProviderKind, endpoint_is_loopback};

use crate::app::App;

/// One place the configuration can send a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub what: String,
    pub endpoint: String,
    pub loopback: bool,
    pub when: String,
}

/// Every destination of the configuration: the model slots in name order, then the optional services.
pub fn destinations(config: &Config) -> Vec<Destination> {
    let mut out: Vec<Destination> = config
        .models
        .iter()
        .map(|(slot, model)| Destination {
            what: format!("model slot `{slot}` ({})", model.id),
            endpoint: model.endpoint.clone(),
            loopback: endpoint_is_loopback(&model.endpoint),
            when: "every request to the slot".to_owned(),
        })
        .collect();
    if config.external.jev.enabled {
        let endpoint = config.external.jev.endpoint.clone().unwrap_or_default();
        let selected = config.decisions.provider == DecisionProviderKind::Jev;
        out.push(Destination {
            what: "Jev decision service".to_owned(),
            loopback: endpoint_is_loopback(&endpoint),
            endpoint,
            when: if selected {
                "stalled loops (decisions.provider = jev)".to_owned()
            } else {
                "none: decisions.provider is not jev".to_owned()
            },
        });
    }
    if config.web.enabled {
        let tools = match config.security.network {
            harness_core::config::Verdict::Allow => "the model's web tools are allowed",
            _ => {
                "the model's web tools are blocked by security.network; `bridle web` works for you"
            }
        };
        for (name, p) in config.web.providers.iter().filter(|(_, p)| p.enabled) {
            let endpoint = p
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.search.brave.com".to_owned());
            out.push(Destination {
                what: format!("web search provider `{name}`"),
                loopback: endpoint_is_loopback(&endpoint),
                endpoint,
                when: format!("every web search ({tools})"),
            });
        }
        out.push(Destination {
            what: "any public web address".to_owned(),
            endpoint: "http(s)://* (not this machine or its network)".to_owned(),
            loopback: false,
            when: format!("web_open and web_download ({tools}); every redirect is checked first"),
        });
        if config.web.browser.enabled {
            out.push(Destination {
                what: "any public web address, through the managed browser (Chromium)".to_owned(),
                endpoint: "http(s)://* and WebSockets (not this machine or its network), only through Bridle's egress proxy on 127.0.0.1".to_owned(),
                loopback: false,
                when: format!(
                    "browser_navigate and browser_action ({tools}); every request the page makes is judged by the proxy, so are redirects, frames, scripts and WebSockets; downloads {}",
                    if config.web.browser.downloads_enabled { "are kept" } else { "are refused" }
                ),
            });
        }
    }
    out
}

/// `bridle egress`: prints the destinations, the proxy state, and what the harness does not control.
pub fn run(app: &App) -> Result<ExitCode> {
    println!("Outbound destinations of this configuration:");
    for row in destinations(&app.config) {
        let scope = if row.loopback { "local " } else { "REMOTE" };
        println!("  {scope} {} - {} ({})", row.endpoint, row.what, row.when);
    }
    let route = crate::app::remote_route_of(&app.config, &|name| {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    });
    println!(
        "Remote requests go through: {} (set it with `network.proxy_mode` in the machine config, or in the window's Info view; loopback never uses a proxy)",
        route.describe()
    );
    if app.config.web.enabled && app.config.web.browser.enabled {
        let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        match harness_runtime::web::browser::proxy::Upstream::from_route(&route, &env) {
            Ok(upstream) => println!(
                "The managed browser reaches the web: {}.",
                upstream.describe(app.config.web.browser.allow_unpinned_destinations)
            ),
            Err(error) => {
                println!("The managed browser cannot start with this proxy setting: {error}")
            }
        }
    }
    println!("Not controlled by the harness:");
    println!("  OpenCode fetches its model catalog at startup (models.dev).");
    println!("  Model weights are downloaded once, during installation.");
    println!("Denied by policy: the webfetch and websearch tools (security.md, network).");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const PROFILE: &str = include_str!("../../../../configs/local-only.toml");

    #[test]
    fn the_shipped_local_profile_reaches_only_loopback() {
        let config: Config = toml::from_str(PROFILE).unwrap();
        let rows = destinations(&config);
        assert!(!rows.is_empty(), "the profile has model slots");
        assert!(rows.iter().all(|row| row.loopback), "{rows:#?}");
    }

    #[test]
    fn a_remote_jev_endpoint_is_listed_as_remote_with_its_condition() {
        let mut config: Config = toml::from_str(PROFILE).unwrap();
        config.external.jev.enabled = true;
        config.external.jev.endpoint = Some("https://jev.example.com".to_owned());
        let jev = destinations(&config)
            .into_iter()
            .find(|row| row.what.starts_with("Jev"))
            .expect("a Jev row");
        assert!(!jev.loopback);
        assert!(jev.when.starts_with("none"), "not selected: {}", jev.when);
        config.decisions.provider = DecisionProviderKind::Jev;
        let jev = destinations(&config)
            .into_iter()
            .find(|row| row.what.starts_with("Jev"))
            .expect("a Jev row");
        assert!(jev.when.starts_with("stalled loops"), "{}", jev.when);
    }
}
