//! Search providers behind one interface. A provider turns a query into ranked hits; it never decides what the model may do.
//! The API key of a provider comes from an environment variable that the configuration names (never from a file), and it is sent
//! only in a request header: it is not in an error message, a log line, or a result.

use std::time::Duration;

use harness_core::config::{WebProviderKind, WebSection, endpoint_is_loopback};

use crate::http::{self, RemoteRoute};

use super::USER_AGENT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Web,
    News,
}

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub kind: Kind,
    pub limit: usize,
    /// Restrict to these sites (`arxiv.org`).
    pub domains: Vec<String>,
    /// Only results published on or after this date, `YYYY-MM-DD`. Providers apply it coarsely (a day, a week, a month, a year).
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
    /// When the provider says it was published or last updated, as the provider wrote it.
    pub published: Option<String>,
}

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, request: &SearchRequest) -> Result<Vec<SearchHit>, String>;
}

/// Days since 1970-01-01 of a civil date (the algorithm of Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// How far back a `YYYY-MM-DD` date is, as the coarse bucket providers offer: `day`, `week`, `month`, `year`, or `None` (older, or
/// not a date).
pub fn freshness_bucket(after: &str, today_days: i64) -> Option<&'static str> {
    let mut parts = after.split('-').map(str::parse::<i64>);
    let (Some(Ok(y)), Some(Ok(m)), Some(Ok(d)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let age = today_days - days_from_civil(y, m, d);
    match age {
        i64::MIN..=1 => Some("day"),
        2..=7 => Some("week"),
        8..=31 => Some("month"),
        32..=366 => Some("year"),
        _ => None,
    }
}

fn today_days() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400),
    )
    .unwrap_or(0)
}

fn with_domains(query: &str, domains: &[String]) -> String {
    let sites: Vec<String> = domains
        .iter()
        .filter(|d| {
            !d.is_empty()
                && d.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
        .map(|d| format!("site:{d}"))
        .collect();
    if sites.is_empty() {
        query.to_owned()
    } else {
        format!("{query} ({})", sites.join(" OR "))
    }
}

fn text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// A self-hosted SearXNG with `search.formats` including `json` in its settings.
#[derive(Debug)]
pub struct Searxng {
    name: String,
    base: String,
    agent: ureq::Agent,
}

impl SearchProvider for Searxng {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, request: &SearchRequest) -> Result<Vec<SearchHit>, String> {
        let mut call = self
            .agent
            .get(&format!("{}/search", self.base))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .query("q", with_domains(&request.query, &request.domains))
            .query("format", "json")
            .query(
                "categories",
                if request.kind == Kind::News {
                    "news"
                } else {
                    "general"
                },
            );
        if let Some(bucket) = request
            .after
            .as_deref()
            .and_then(|a| freshness_bucket(a, today_days()))
        {
            call = call.query("time_range", bucket);
        }
        let mut response = call
            .call()
            .map_err(|e| format!("{} could not be reached: {e}", self.name))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!(
                "{} answered HTTP {status}{}",
                self.name,
                if status == 403 {
                    " (its JSON output may be disabled: search.formats must include json)"
                } else {
                    ""
                }
            ));
        }
        let body: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("{} answered something that is not JSON: {e}", self.name))?;
        let results = body
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("{} answered JSON without a results list", self.name))?;
        Ok(results
            .iter()
            .filter(|r| !text(r, "url").is_empty())
            .take(request.limit)
            .map(|r| SearchHit {
                title: text(r, "title"),
                url: text(r, "url"),
                snippet: text(r, "content"),
                provider: self.name.clone(),
                published: Some(text(r, "publishedDate")).filter(|p| !p.is_empty()),
            })
            .collect())
    }
}

/// The Brave Search API.
pub struct Brave {
    name: String,
    base: String,
    key: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Brave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Brave")
            .field("name", &self.name)
            .field("base", &self.base)
            .field("key", &"<hidden>")
            .finish()
    }
}

impl SearchProvider for Brave {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, request: &SearchRequest) -> Result<Vec<SearchHit>, String> {
        let path = if request.kind == Kind::News {
            "news/search"
        } else {
            "web/search"
        };
        let mut call = self
            .agent
            .get(&format!("{}/res/v1/{path}", self.base))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.key)
            .query("q", with_domains(&request.query, &request.domains))
            .query("count", request.limit.min(20).to_string());
        if let Some(bucket) = request
            .after
            .as_deref()
            .and_then(|a| freshness_bucket(a, today_days()))
        {
            call = call.query(
                "freshness",
                match bucket {
                    "day" => "pd",
                    "week" => "pw",
                    "month" => "pm",
                    _ => "py",
                },
            );
        }
        let mut response = call
            .call()
            .map_err(|e| format!("{} could not be reached: {e}", self.name))?;
        let status = response.status().as_u16();
        match status {
            200 => {}
            401 | 403 => {
                return Err(format!(
                    "{} refused the API key (HTTP {status}); check the key in the environment variable the configuration names",
                    self.name
                ));
            }
            429 => {
                return Err(format!(
                    "{} is rate limiting this key (HTTP 429); try again later",
                    self.name
                ));
            }
            _ => return Err(format!("{} answered HTTP {status}", self.name)),
        }
        let body: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("{} answered something that is not JSON: {e}", self.name))?;
        let results = if request.kind == Kind::News {
            body.get("results")
        } else {
            body.pointer("/web/results")
        };
        let results = results
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(results
            .iter()
            .filter(|r| !text(r, "url").is_empty())
            .take(request.limit)
            .map(|r| SearchHit {
                title: text(r, "title"),
                url: text(r, "url"),
                snippet: text(r, "description"),
                provider: self.name.clone(),
                published: Some(text(r, "page_age"))
                    .filter(|p| !p.is_empty())
                    .or_else(|| Some(text(r, "age")).filter(|p| !p.is_empty())),
            })
            .collect())
    }
}

/// The enabled providers of the configuration, in name order. A provider that cannot work (a missing key) is left out, with the
/// reason in the second value, so the caller can say why.
pub fn build(
    web: &WebSection,
    route: &RemoteRoute,
    env: &dyn Fn(&str) -> Option<String>,
) -> (Vec<Box<dyn SearchProvider>>, Vec<String>) {
    let timeout = Duration::from_millis(web.timeout_ms);
    let mut providers: Vec<Box<dyn SearchProvider>> = Vec::new();
    let mut skipped = Vec::new();
    for (name, p) in web.providers.iter().filter(|(_, p)| p.enabled) {
        match p.kind {
            WebProviderKind::Searxng => {
                let base = p
                    .base_url
                    .clone()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .to_owned();
                let agent = if endpoint_is_loopback(&base) {
                    http::local_agent(timeout)
                } else {
                    http::agent_for(route, timeout)
                };
                providers.push(Box::new(Searxng {
                    name: name.clone(),
                    base,
                    agent,
                }));
            }
            WebProviderKind::Brave => {
                let var = p.api_key_env.clone().unwrap_or_default();
                match env(&var).filter(|k| !k.trim().is_empty()) {
                    Some(key) => {
                        let base = p
                            .base_url
                            .clone()
                            .unwrap_or_else(|| "https://api.search.brave.com".to_owned())
                            .trim_end_matches('/')
                            .to_owned();
                        let agent = if endpoint_is_loopback(&base) {
                            http::local_agent(timeout)
                        } else {
                            http::agent_for(route, timeout)
                        };
                        providers.push(Box::new(Brave {
                            name: name.clone(),
                            base,
                            key: key.trim().to_owned(),
                            agent,
                        }));
                    }
                    None => {
                        skipped.push(format!("{name}: the environment variable {var} is not set"))
                    }
                }
            }
        }
    }
    (providers, skipped)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use harness_core::config::WebProvider;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A server that answers each request with the next canned `(status, body)`; it records the request heads.
    fn serve(replies: Vec<(u16, String)>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0u8; 4096];
                let n = stream.read(&mut buffer).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buffer[..n]).into_owned());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_owned(),
            kind: Kind::Web,
            limit: 5,
            domains: vec![],
            after: None,
        }
    }

    fn web_with(kind: WebProviderKind, name: &str, base: &str, env: Option<&str>) -> WebSection {
        let mut web = WebSection {
            enabled: true,
            ..WebSection::default()
        };
        web.providers.insert(
            name.to_owned(),
            WebProvider {
                kind,
                enabled: true,
                base_url: Some(base.to_owned()),
                api_key_env: env.map(str::to_owned),
            },
        );
        web
    }

    #[test]
    fn dates_become_coarse_freshness_buckets() {
        let today = days_from_civil(2026, 11, 4);
        assert_eq!(freshness_bucket("2026-11-04", today), Some("day"));
        assert_eq!(freshness_bucket("2026-11-01", today), Some("week"));
        assert_eq!(freshness_bucket("2026-10-20", today), Some("month"));
        assert_eq!(freshness_bucket("2026-03-01", today), Some("year"));
        assert_eq!(freshness_bucket("2020-01-01", today), None);
        for bad in ["yesterday", "2026-13-01", "2026-11", "2026-11-04-1"] {
            assert_eq!(freshness_bucket(bad, today), None, "{bad}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn searxng_results_are_read_and_the_query_carries_the_sites_and_the_format() {
        let body = r#"{"results":[{"title":"A","url":"https://a.org/x","content":"snip","publishedDate":"2026-10-30"},{"title":"no url","content":"x"},{"title":"B","url":"https://b.org/","content":""}]}"#;
        let (base, heads) = serve(vec![(200, body.to_owned())]);
        let (providers, skipped) = build(
            &web_with(WebProviderKind::Searxng, "local", &base, None),
            &RemoteRoute::Direct,
            &|_| None,
        );
        assert!(skipped.is_empty());
        let mut req = request("rust async");
        req.domains = vec!["docs.rs".into(), "bad domain!".into()];
        req.kind = Kind::News;
        let hits = providers[0].search(&req).unwrap();
        assert_eq!(hits.len(), 2, "a result with no address is dropped");
        assert_eq!(hits[0].published.as_deref(), Some("2026-10-30"));
        assert_eq!(hits[0].provider, "local");
        let head = heads.recv().unwrap();
        assert!(
            head.contains("format=json")
                && head.contains("categories=news")
                && head.contains("site%3Adocs.rs"),
            "{head}"
        );
        assert!(
            !head.contains("bad%20domain"),
            "an invalid site is not sent"
        );
    }

    #[test]
    fn brave_sends_the_key_in_a_header_only_and_reads_web_and_news_shapes() {
        let web = r#"{"web":{"results":[{"title":"W","url":"https://w.org/","description":"d","page_age":"2026-10-01T00:00:00"}]}}"#;
        let news = r#"{"results":[{"title":"N","url":"https://n.org/","description":"fresh","age":"3 hours ago"}]}"#;
        let (base, heads) = serve(vec![(200, web.to_owned()), (200, news.to_owned())]);
        let cfg = web_with(WebProviderKind::Brave, "brave", &base, Some("BRAVE_KEY"));
        let (providers, _) = build(&cfg, &RemoteRoute::Direct, &|n| {
            (n == "BRAVE_KEY").then(|| "secret-key-123".to_owned())
        });
        let hits = providers[0].search(&request("q")).unwrap();
        assert_eq!(
            (hits[0].title.as_str(), hits[0].published.as_deref()),
            ("W", Some("2026-10-01T00:00:00"))
        );
        let head = heads.recv().unwrap();
        assert!(
            head.contains("x-subscription-token: secret-key-123")
                || head.contains("X-Subscription-Token: secret-key-123"),
            "{head}"
        );
        assert!(
            !head.contains("secret-key-123 HTTP")
                && !head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .contains("secret"),
            "the key is not in the address"
        );
        let mut req = request("q");
        req.kind = Kind::News;
        req.after = Some(chrono_free_recent());
        let hits = providers[0].search(&req).unwrap();
        assert_eq!(hits[0].published.as_deref(), Some("3 hours ago"));
        assert!(heads.recv().unwrap().contains("/res/v1/news/search"));
    }

    fn chrono_free_recent() -> String {
        let days = today_days();
        // A date a few days ago, written without a date library.
        let z = days - 3 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}")
    }

    #[test]
    fn provider_failures_are_reported_in_words_and_never_contain_the_key() {
        let (base, _) = serve(vec![
            (401, "{}".into()),
            (429, "{}".into()),
            (200, "not json".into()),
            (200, r#"{"nope":1}"#.into()),
        ]);
        let cfg = web_with(WebProviderKind::Brave, "brave", &base, Some("K"));
        let (providers, _) = build(&cfg, &RemoteRoute::Direct, &|_| {
            Some("hunter2-secret".to_owned())
        });
        let errors: Vec<String> = (0..2)
            .map(|_| providers[0].search(&request("q")).unwrap_err())
            .collect();
        assert!(errors[0].contains("refused the API key"), "{}", errors[0]);
        assert!(errors[1].contains("rate limiting"), "{}", errors[1]);
        assert!(
            providers[0].search(&request("q")).is_err(),
            "invalid JSON is an error"
        );
        assert!(
            providers[0].search(&request("q")).unwrap().is_empty(),
            "a Brave answer with no results is an empty list"
        );
        for e in &errors {
            assert!(!e.contains("hunter2"), "{e}");
        }
        let (searx, _) = serve(vec![(403, "{}".into()), (200, "{\"nope\":1}".into())]);
        let (p, _) = build(
            &web_with(WebProviderKind::Searxng, "s", &searx, None),
            &RemoteRoute::Direct,
            &|_| None,
        );
        assert!(
            p[0].search(&request("q"))
                .unwrap_err()
                .contains("JSON output may be disabled")
        );
        assert!(
            p[0].search(&request("q"))
                .unwrap_err()
                .contains("without a results list")
        );
    }

    #[test]
    fn a_brave_provider_without_its_key_is_left_out_with_the_reason() {
        let cfg = web_with(
            WebProviderKind::Brave,
            "brave",
            "https://api.search.brave.com",
            Some("BRAVE_KEY"),
        );
        let (providers, skipped) = build(&cfg, &RemoteRoute::Direct, &|_| None);
        assert!(providers.is_empty());
        assert_eq!(
            skipped,
            ["brave: the environment variable BRAVE_KEY is not set"]
        );
        let (providers, _) = build(&cfg, &RemoteRoute::Direct, &|_| Some("   ".into()));
        assert!(providers.is_empty(), "a blank key is no key");
    }
}
