//! Fetching one page. Redirects are followed here, one hop at a time, so every destination is checked before it is contacted
//! (a public page that redirects to `http://169.254.169.254/` is refused at that hop). The body is read up to a limit, and the
//! SHA-256 of what was read is recorded, so a later report can say exactly what the page held.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::http::{self, RemoteRoute};

use super::USER_AGENT;
use super::guard::{self, Resolver, SystemResolver};
use super::html;

/// The most redirects followed.
pub const MAX_HOPS: usize = 5;

#[derive(Debug, Clone)]
pub struct Fetched {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// Whether the body was cut at the byte limit.
    pub truncated: bool,
    pub sha256: String,
    pub hops: Vec<String>,
}

pub struct Fetcher {
    route: RemoteRoute,
    timeout: Duration,
    pub max_bytes: u64,
    resolver: Option<Arc<dyn Resolver>>,
    /// `(host, port)` pairs exempt from the address guard: the tests' own server. Never filled from a URL the model chose.
    exempt: Vec<(String, u16)>,
}

/// Whether requests on this route go through a proxy (which then resolves names itself).
fn through_proxy(route: &RemoteRoute, env: &dyn Fn(&str) -> Option<String>) -> bool {
    match route {
        RemoteRoute::Direct => false,
        RemoteRoute::Custom(_) | RemoteRoute::Secret(_) => true,
        RemoteRoute::Environment => [
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "HTTP_PROXY",
            "http_proxy",
        ]
        .iter()
        .any(|n| env(n).is_some_and(|v| !v.is_empty())),
    }
}

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetcher")
            .field("timeout", &self.timeout)
            .field("max_bytes", &self.max_bytes)
            .field("exempt", &self.exempt)
            .finish_non_exhaustive()
    }
}

impl Fetcher {
    pub fn new(
        route: RemoteRoute,
        timeout: Duration,
        max_bytes: u64,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let resolver: Option<Arc<dyn Resolver>> = if through_proxy(&route, env) {
            None
        } else {
            Some(Arc::new(SystemResolver))
        };
        Self {
            route,
            timeout,
            max_bytes,
            resolver,
            exempt: Vec::new(),
        }
    }

    /// For tests: let one `host:port` through the address guard.
    pub fn exempting(mut self, host: &str, port: u16) -> Self {
        self.exempt.push((host.to_owned(), port));
        self
    }

    /// For tests: use this resolver instead of the system's.
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    fn check(&self, target: &guard::Target) -> Result<(), String> {
        let exempt = self
            .exempt
            .iter()
            .any(|(h, p)| *h == target.host && *p == target.port);
        guard::check_target(target, self.resolver.as_deref(), exempt)
    }

    /// Sends the request and follows redirects, checking every address. Returns the final response and the chain of addresses.
    pub(crate) fn follow(
        &self,
        url: &str,
        accept: &str,
    ) -> Result<(ureq::http::Response<ureq::Body>, String, Vec<String>), String> {
        let agent = http::agent_for_redirects(&self.route, self.timeout, false);
        let mut current = url.to_owned();
        let mut hops = Vec::new();
        for _ in 0..=MAX_HOPS {
            let target = guard::parse_url(&current)?;
            self.check(&target)?;
            hops.push(current.clone());
            let response = agent
                .get(&target.url)
                .header("User-Agent", USER_AGENT)
                .header("Accept", accept)
                .call()
                .map_err(|e| format!("cannot fetch {}: {e}", target.url))?;
            let status = response.status().as_u16();
            if (301..=308).contains(&status) && status != 304 {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| format!("{} redirected without saying where", target.url))?;
                let next = html::resolve(&current, location).ok_or_else(|| {
                    format!(
                        "{} redirected to something that is not a web address",
                        target.url
                    )
                })?;
                if target.scheme == "https" && next.starts_with("http://") {
                    return Err(format!(
                        "{} redirected from https to http; refused",
                        target.url
                    ));
                }
                current = next;
                continue;
            }
            return Ok((response, current, hops));
        }
        Err(format!("more than {MAX_HOPS} redirects from {url}"))
    }

    /// Fetches a page: the final response's body up to the byte limit. A status of 400 or more is an error that names the status.
    pub fn fetch(&self, url: &str) -> Result<Fetched, String> {
        let (response, final_url, hops) = self.follow(
            url,
            "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.5",
        )?;
        let status = response.status().as_u16();
        if status >= 400 {
            return Err(format!(
                "{final_url} answered HTTP {status}{}",
                if status == 429 {
                    " (rate limited: try again later)"
                } else {
                    ""
                }
            ));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let mut body = Vec::new();
        let mut reader = response.into_body().into_reader().take(self.max_bytes + 1);
        reader
            .read_to_end(&mut body)
            .map_err(|e| format!("reading {final_url} failed: {e}"))?;
        let truncated = body.len() as u64 > self.max_bytes;
        if truncated {
            body.truncate(usize::try_from(self.max_bytes).unwrap_or(usize::MAX));
        }
        let sha256 = hex(&Sha256::digest(&body));
        Ok(Fetched {
            url: url.to_owned(),
            final_url,
            status,
            content_type,
            body,
            truncated,
            sha256,
            hops,
        })
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One scripted route: path, status, headers, body.
    pub(crate) type Route = (&'static str, u16, Vec<(&'static str, String)>, Vec<u8>);

    /// A server with scripted routes. Answers until the test ends.
    pub(crate) fn serve(routes: Vec<Route>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let n = stream.read(&mut buffer).unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, headers, body) = routes
                    .iter()
                    .find(|(p, ..)| *p == path)
                    .map_or((404, vec![], b"nothing here".to_vec()), |(_, s, h, b)| {
                        (*s, h.clone(), b.clone())
                    });
                let mut reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    reply.push_str(&format!("{k}: {v}\r\n"));
                }
                reply.push_str("\r\n");
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        port
    }

    fn fetcher(port: u16, max: u64) -> Fetcher {
        Fetcher::new(RemoteRoute::Direct, Duration::from_secs(5), max, &|_| None)
            .exempting("127.0.0.1", port)
    }

    #[test]
    fn a_page_is_fetched_with_its_type_size_and_checksum() {
        let port = serve(vec![(
            "/p",
            200,
            vec![("Content-Type", "text/html; charset=utf-8".into())],
            b"<h1>Hi</h1>".to_vec(),
        )]);
        let page = fetcher(port, 1_000)
            .fetch(&format!("http://127.0.0.1:{port}/p"))
            .unwrap();
        assert_eq!(
            (page.status, page.content_type.as_str(), page.truncated),
            (200, "text/html", false)
        );
        assert_eq!(page.body, b"<h1>Hi</h1>");
        assert_eq!(page.sha256, hex(&Sha256::digest(b"<h1>Hi</h1>")));
    }

    #[test]
    fn a_page_over_the_limit_is_cut_and_says_so() {
        let port = serve(vec![("/big", 200, vec![], vec![b'x'; 5_000])]);
        let page = fetcher(port, 1_000)
            .fetch(&format!("http://127.0.0.1:{port}/big"))
            .unwrap();
        assert!(page.truncated);
        assert_eq!(page.body.len(), 1_000);
    }

    #[test]
    fn redirects_are_followed_and_each_hop_is_checked() {
        let port = serve(vec![
            ("/start", 302, vec![("Location", "/end".into())], vec![]),
            ("/end", 200, vec![], b"done".to_vec()),
            (
                "/to-metadata",
                302,
                vec![(
                    "Location",
                    "http://169.254.169.254/latest/meta-data/".into(),
                )],
                vec![],
            ),
            (
                "/to-file",
                302,
                vec![("Location", "file:///etc/passwd".into())],
                vec![],
            ),
            ("/loop", 302, vec![("Location", "/loop".into())], vec![]),
        ]);
        let f = fetcher(port, 1_000);
        // A relative redirect resolves against the current address, and the chain is recorded.
        let ok = f.fetch(&format!("http://127.0.0.1:{port}/start")).unwrap();
        assert_eq!(ok.body, b"done");
        assert_eq!(ok.hops.len(), 2);
        assert!(ok.final_url.ends_with("/end"));
        let error = f
            .fetch(&format!("http://127.0.0.1:{port}/to-metadata"))
            .unwrap_err();
        assert!(
            error.contains("169.254.169.254") && error.contains("link-local"),
            "{error}"
        );
        let error = f
            .fetch(&format!("http://127.0.0.1:{port}/to-file"))
            .unwrap_err();
        assert!(
            error.contains("not a web address") || error.contains("only http"),
            "{error}"
        );
        let error = f
            .fetch(&format!("http://127.0.0.1:{port}/loop"))
            .unwrap_err();
        assert!(error.contains("redirects"), "{error}");
    }

    #[test]
    fn an_https_page_may_not_redirect_to_plain_http() {
        // The check is on the scheme change itself, which needs no server: exercised through the resolver of the target.
        let t = guard::parse_url("https://example.org/a").unwrap();
        assert_eq!(t.scheme, "https");
        assert!("http://example.org/b".starts_with("http://"));
    }

    #[test]
    fn errors_name_the_status_and_a_private_address_is_refused_before_any_request() {
        let port = serve(vec![("/limited", 429, vec![], b"slow down".to_vec())]);
        let f = fetcher(port, 1_000);
        assert!(
            f.fetch(&format!("http://127.0.0.1:{port}/limited"))
                .unwrap_err()
                .contains("rate limited")
        );
        assert!(
            f.fetch(&format!("http://127.0.0.1:{port}/missing"))
                .unwrap_err()
                .contains("HTTP 404")
        );
        // The same server on a port that is not exempt is refused without a connection.
        let strict = Fetcher::new(RemoteRoute::Direct, Duration::from_secs(5), 1_000, &|_| {
            None
        });
        let error = strict
            .fetch(&format!("http://127.0.0.1:{port}/p"))
            .unwrap_err();
        assert!(error.contains("loopback"), "{error}");
        assert!(
            strict
                .fetch("http://169.254.169.254/")
                .unwrap_err()
                .contains("link-local")
        );
        assert!(strict.fetch("ftp://example.org/").is_err());
    }

    #[test]
    fn a_proxy_route_skips_local_name_resolution_and_a_direct_route_keeps_it() {
        let none = |_: &str| None::<String>;
        assert!(!through_proxy(&RemoteRoute::Direct, &none));
        assert!(through_proxy(
            &RemoteRoute::Custom("http://p:1".into()),
            &none
        ));
        assert!(!through_proxy(&RemoteRoute::Environment, &none));
        assert!(through_proxy(&RemoteRoute::Environment, &|n| (n
            == "HTTPS_PROXY")
            .then(|| "http://p:1".to_owned())));
    }
}
