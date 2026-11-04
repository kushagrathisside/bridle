//! HTTP clients for the model servers (loopback) and for the optional Jev service (remote).
//!
//! `local_agent` never uses a proxy: a proxy would intercept localhost traffic. `remote_agent` takes the route the app
//! was told to use ([`set_remote_route`], from `network.proxy_mode` and the `BRIDLE_PROXY` variable): the proxy the
//! environment names (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`) by default, as every other program on the machine does, a
//! proxy named in the configuration, or none. It checks certificates with the system store through native-tls (SChannel
//! on Windows, OpenSSL elsewhere), so a corporate root certificate in the store is trusted. See ADR-0007 and ADR-0018.

use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

/// Where requests to remote endpoints go.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RemoteRoute {
    /// The proxy the environment names, if any. The default.
    #[default]
    Environment,
    /// A direct connection, whatever the environment says.
    Direct,
    /// A proxy from the configuration. Its URL carries no credentials and may be shown.
    Custom(String),
    /// A proxy from the `BRIDLE_PROXY` variable. It may carry credentials, so it is never shown or logged.
    Secret(String),
}

impl RemoteRoute {
    /// The route in words, without anything secret in it.
    pub fn describe(&self) -> String {
        match self {
            Self::Environment => {
                let named: Vec<&str> = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"]
                    .into_iter()
                    .filter(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
                    .collect();
                if named.is_empty() {
                    "direct (the environment names no proxy)".to_owned()
                } else {
                    format!(
                        "the environment's proxy ({} set; value hidden)",
                        named.join(", ")
                    )
                }
            }
            Self::Direct => "direct (no proxy)".to_owned(),
            Self::Custom(url) => format!("the proxy {url}"),
            Self::Secret(_) => "the proxy in BRIDLE_PROXY (value hidden)".to_owned(),
        }
    }
}

static ROUTE: RwLock<RemoteRoute> = RwLock::new(RemoteRoute::Environment);

/// Sets the route every later [`remote_agent`] uses. The app calls it whenever it loads the configuration.
pub fn set_remote_route(route: RemoteRoute) {
    *ROUTE.write().unwrap_or_else(PoisonError::into_inner) = route;
}

/// The route [`remote_agent`] uses now.
pub fn remote_route() -> RemoteRoute {
    ROUTE.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Builds an agent for the model servers on loopback: no proxy, the given whole-request timeout, and non-2xx
/// statuses returned rather than raised.
pub fn local_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

/// Builds an agent for a remote endpoint on the current route ([`remote_route`]), with the system certificate store,
/// the given whole-request timeout, and non-2xx statuses returned rather than raised.
pub fn remote_agent(timeout: Duration) -> ureq::Agent {
    agent_for(&remote_route(), timeout)
}

/// Builds a remote agent for `route`. A proxy URL that cannot be parsed falls back to a direct connection and is reported
/// by the configuration check (V47), not here.
pub fn agent_for(route: &RemoteRoute, timeout: Duration) -> ureq::Agent {
    let builder = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        );
    let config = match route {
        RemoteRoute::Environment => builder.build(),
        RemoteRoute::Direct => builder.proxy(None).build(),
        RemoteRoute::Custom(url) | RemoteRoute::Secret(url) => {
            builder.proxy(ureq::Proxy::new(url).ok()).build()
        }
    };
    config.into()
}

/// What a connection test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub status: u16,
    pub elapsed_ms: u64,
    pub route: String,
}

/// Sends one `HEAD` request to `url` over `route` and reports the status and the time. The person asked for it (the
/// "Test" button); nothing else in the app calls it.
pub fn probe(route: &RemoteRoute, url: &str, timeout: Duration) -> Result<Probe, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("the test address must start with http:// or https://".to_owned());
    }
    let started = Instant::now();
    let response = agent_for(route, timeout)
        .head(url)
        .call()
        .map_err(|error| format!("{error}"))?;
    Ok(Probe {
        status: response.status().as_u16(),
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        route: route.describe(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn both_agents_are_built_and_can_be_cloned() {
        let local = local_agent(Duration::from_secs(1));
        let _local_again = local.clone();
        let remote = remote_agent(Duration::from_secs(1));
        let _remote_again = remote.clone();
    }

    /// Reads one request head from `stream` and returns its first line.
    fn read_head(stream: &mut std::net::TcpStream) -> String {
        let mut head = Vec::new();
        let mut buffer = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buffer[..n]),
            }
        }
        String::from_utf8_lossy(&head)
            .lines()
            .next()
            .unwrap_or("")
            .to_owned()
    }

    /// A tiny proxy. It accepts the `CONNECT` tunnel the client opens, then answers the request inside it with 204, and
    /// sends both request lines to the test. It closes politely: dropping a socket that still holds unread bytes sends a
    /// reset on Windows, and the client would see that instead of the answer.
    fn fake_proxy() -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let _ = tx.send(read_head(&mut stream));
                let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                let _ = tx.send(read_head(&mut stream));
                let _ = stream.write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let mut rest = Vec::new();
                let _ = stream.read_to_end(&mut rest);
            }
        });
        (port, rx)
    }

    #[test]
    fn a_custom_route_sends_the_request_to_the_proxy_not_to_the_host() {
        let (port, seen) = fake_proxy();
        // The host does not exist: only a proxy can answer.
        let route = RemoteRoute::Custom(format!("http://127.0.0.1:{port}"));
        let probe = probe(
            &route,
            "http://proxytest.invalid/ping",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(probe.status, 204);
        assert!(
            probe.route.contains(&format!("127.0.0.1:{port}")),
            "{}",
            probe.route
        );
        let tunnel = seen.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            tunnel.starts_with("CONNECT proxytest.invalid:80"),
            "{tunnel}"
        );
        let request = seen.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("HEAD /ping"), "{request}");
    }

    #[test]
    fn the_secret_route_works_and_its_description_hides_the_url() {
        let (port, _seen) = fake_proxy();
        let route = RemoteRoute::Secret(format!("http://user:hunter2@127.0.0.1:{port}"));
        assert!(!route.describe().contains("hunter2"));
        let probe = probe(
            &route,
            "http://proxytest.invalid/ping",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(probe.status, 204);
    }

    #[test]
    fn the_direct_route_ignores_a_proxy_and_cannot_reach_a_host_that_does_not_exist() {
        let error = probe(
            &RemoteRoute::Direct,
            "http://proxytest.invalid/ping",
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn the_test_address_must_be_http_or_https() {
        assert!(
            probe(
                &RemoteRoute::Direct,
                "file:///etc/passwd",
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(probe(&RemoteRoute::Direct, "ftp://host/x", Duration::from_secs(1)).is_err());
    }

    #[test]
    fn the_route_is_set_once_and_read_back() {
        let before = remote_route();
        set_remote_route(RemoteRoute::Direct);
        assert_eq!(remote_route(), RemoteRoute::Direct);
        set_remote_route(before);
    }
}
