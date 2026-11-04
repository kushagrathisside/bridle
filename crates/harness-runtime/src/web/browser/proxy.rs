//! The egress proxy: the only way out of the browser (ADR-0020).
//!
//! Chromium is started with this proxy as its only route (and with name lookup switched off outside it), so **every** connection the
//! browser makes, for the page, its scripts, images, frames, WebSockets and redirects, arrives here and is judged here. The
//! judgement is the one the plain web tools use (`guard::resolve_pinned`): no loopback, private, link-local (cloud metadata),
//! shared or reserved addresses, in any spelling, and no local names.
//!
//! **DNS rebinding.** When Bridle connects directly, this proxy resolves the name itself, **once**, judges every answer, and then
//! connects to one of the addresses it judged, never to the name again. A page cannot make the name mean something else between the
//! check and the connection, because there is no second lookup. When the machine reaches the web through an upstream proxy (the
//! only way out on some networks) the upstream resolves names, so the address cannot be pinned here: the proxy then refuses
//! numbers and local names and forwards the rest by name, and the upstream proxy is the boundary for what a name resolves to. That
//! difference is shown by `bridle egress` and `bridle web browser status`.
//!
//! Each browser session has its own credentials (`Proxy-Authorization`), so a connection is attributed to a session, another
//! program on the machine cannot use the proxy, and a finished session's access ends at once. The proxy keeps, for each session,
//! the hosts it contacted, what it refused and why, and how many bytes moved; and it cuts a call that moves more bytes than its
//! limit, which bounds what a download can write to disk.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::http::RemoteRoute;
use crate::web::guard::{self, Resolver};

const MAX_HEAD: usize = 16 * 1024;
const MAX_HEADERS: usize = 100;
const MAX_CONNECTIONS: usize = 96;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_LIMIT: Duration = Duration::from_secs(120);
const MAX_HOSTS: usize = 200;
const MAX_BLOCKED: usize = 50;
/// Ports of services that are not web servers. Chromium refuses most of them anyway; this is the second lock.
const DENIED_PORTS: &[u16] = &[
    21, 22, 23, 25, 110, 111, 135, 137, 139, 143, 389, 445, 465, 587, 993, 995, 2049, 3306, 5432,
    6379, 11211, 27017,
];

/// Where the proxy sends what it lets through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    /// Straight to the destination, at the address the proxy judged.
    Direct,
    /// Through the person's own HTTP proxy, which resolves names.
    Http {
        /// `host:port`.
        address: String,
        /// `Basic` credentials, base64, never shown.
        auth: Option<String>,
    },
}

impl Upstream {
    /// The upstream for a route. A route that names a proxy Bridle cannot speak to is an error: the browser must not silently
    /// go around it.
    pub fn from_route(
        route: &RemoteRoute,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let Some(url) = super::runtime::proxy_url(route, env) else {
            return Ok(Self::Direct);
        };
        Self::parse(&url)
    }

    pub fn parse(url: &str) -> Result<Self, String> {
        let rest = match url.split_once("://") {
            Some(("http", rest)) => rest,
            Some((scheme, _)) => {
                return Err(format!(
                    "the managed browser can use an HTTP proxy only, not `{scheme}://`; set network.proxy_mode or the proxy to an http:// address"
                ));
            }
            None => url,
        };
        let rest = rest.trim_end_matches('/');
        let (credentials, hostport) = match rest.rsplit_once('@') {
            Some((c, h)) => (Some(c), h),
            None => (None, rest),
        };
        if hostport.is_empty()
            || hostport.contains('/')
            || hostport.chars().any(char::is_whitespace)
        {
            return Err("the proxy address is not host:port".to_owned());
        }
        let address = if hostport.contains(':') && !hostport.ends_with(']') {
            hostport.to_owned()
        } else {
            format!("{hostport}:80")
        };
        Ok(Self::Http {
            address,
            auth: credentials.map(|c| super::runtime::base64(c.as_bytes())),
        })
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Self::Direct => {
                "direct: names are resolved here once and the connection goes to the address that was checked"
            }
            Self::Http { .. } => {
                "through your proxy: it resolves names, so the address cannot be pinned here; numbers and local names are still refused"
            }
        }
    }

    pub fn pins_addresses(&self) -> bool {
        matches!(self, Self::Direct)
    }
}

/// Opens connections for the proxy. Replaced in tests.
pub trait Dialer: Send + Sync {
    fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TcpDialer;

impl Dialer for TcpDialer {
    fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        TcpStream::connect_timeout(&address, timeout)
    }
}

/// One refused request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub host: String,
    pub port: u16,
    pub reason: String,
}

/// What passed through for one session.
#[derive(Debug, Default)]
pub struct Traffic {
    bytes: AtomicU64,
    call_bytes: AtomicU64,
    call_limit: AtomicU64,
    connections: AtomicU64,
    /// Every refusal since the session began (never reset), and how many of them have been told to the caller.
    blocked_total: AtomicU64,
    blocked_told: AtomicU64,
    hosts: Mutex<BTreeSet<String>>,
    /// The most recent refusals, newest last.
    blocked: Mutex<Vec<Blocked>>,
}

impl Traffic {
    /// Starts a new call: its byte count starts again from zero, and it may move at most `limit` bytes.
    pub fn begin_call(&self, limit: u64) {
        self.call_bytes.store(0, Ordering::SeqCst);
        self.call_limit.store(limit, Ordering::SeqCst);
    }

    pub fn total_bytes(&self) -> u64 {
        self.bytes.load(Ordering::SeqCst)
    }

    pub fn call_bytes(&self) -> u64 {
        self.call_bytes.load(Ordering::SeqCst)
    }

    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }

    /// The hosts contacted (`host:port`), in order, at most 200.
    pub fn hosts(&self) -> Vec<String> {
        self.hosts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Everything refused since the session began: the exact count, and the most recent 50.
    pub fn blocked(&self) -> (u64, Vec<Blocked>) {
        (
            self.blocked_total.load(Ordering::SeqCst),
            self.blocked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }

    /// What was refused since the last time this was asked: the number, and up to the 5 most recent. A page can ask for forbidden
    /// things between two calls (a timer), so the news is delivered on the next call, not lost.
    pub fn take_blocked(&self) -> (u64, Vec<Blocked>) {
        let total = self.blocked_total.load(Ordering::SeqCst);
        let told = self.blocked_told.swap(total, Ordering::SeqCst);
        let new = total.saturating_sub(told);
        let list = self.blocked.lock().unwrap_or_else(PoisonError::into_inner);
        let take = usize::try_from(new)
            .unwrap_or(usize::MAX)
            .min(list.len())
            .min(5);
        (new, list[list.len() - take..].to_vec())
    }

    fn note_host(&self, host: &str, port: u16) {
        let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        if hosts.len() < MAX_HOSTS {
            hosts.insert(format!("{host}:{port}"));
        }
    }

    fn note_blocked(&self, host: &str, port: u16, reason: &str) {
        self.blocked_total.fetch_add(1, Ordering::SeqCst);
        let mut blocked = self.blocked.lock().unwrap_or_else(PoisonError::into_inner);
        if blocked.len() >= MAX_BLOCKED {
            blocked.remove(0);
        }
        blocked.push(Blocked {
            host: host.to_owned(),
            port,
            reason: reason.to_owned(),
        });
    }

    /// Counts bytes; false when the call has moved more than its limit.
    fn count(&self, n: u64) -> bool {
        self.bytes.fetch_add(n, Ordering::SeqCst);
        let now = self.call_bytes.fetch_add(n, Ordering::SeqCst) + n;
        now <= self.call_limit.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct Entry {
    id: String,
    traffic: Arc<Traffic>,
}

/// How to build a proxy.
pub struct ProxyConfig {
    pub upstream: Upstream,
    pub resolver: Arc<dyn Resolver>,
    pub dialer: Arc<dyn Dialer>,
    /// `(host, port)` pairs that skip the address check: the self-test page that Bridle serves itself, and the tests' own servers.
    /// Set only by Bridle's own code, never from a URL, the configuration or the model.
    pub exempt: Vec<(String, u16)>,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("upstream", &self.upstream)
            .field("exempt", &self.exempt)
            .finish_non_exhaustive()
    }
}

impl ProxyConfig {
    pub fn new(upstream: Upstream) -> Self {
        Self {
            upstream,
            resolver: Arc::new(guard::SystemResolver),
            dialer: Arc::new(TcpDialer),
            exempt: Vec::new(),
        }
    }
}

struct Shared {
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
    stop: AtomicBool,
    active: AtomicUsize,
    config: ProxyConfig,
}

pub struct EgressProxy {
    shared: Arc<Shared>,
    address: SocketAddr,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for EgressProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressProxy")
            .field("address", &self.address)
            .field("upstream", &self.shared.config.upstream)
            .finish_non_exhaustive()
    }
}

impl EgressProxy {
    /// Starts listening on a free port of 127.0.0.1.
    pub fn start(config: ProxyConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            sessions: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            config,
        });
        let for_thread = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("bridle-egress-proxy".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    if for_thread.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    if for_thread.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        for_thread.active.fetch_sub(1, Ordering::SeqCst);
                        respond(
                            &stream,
                            503,
                            "Service Unavailable",
                            "Bridle's egress proxy has too many connections open.",
                            &[],
                        );
                        continue;
                    }
                    let shared = Arc::clone(&for_thread);
                    let spawned = thread::Builder::new()
                        .name("bridle-egress-conn".to_owned())
                        .spawn({
                            let shared = Arc::clone(&shared);
                            move || {
                                handle(&shared, stream);
                                shared.active.fetch_sub(1, Ordering::SeqCst);
                            }
                        });
                    if spawned.is_err() {
                        shared.active.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            })?;
        Ok(Self {
            shared,
            address,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// The address Chromium is told to use.
    pub fn server_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn upstream(&self) -> &Upstream {
        &self.shared.config.upstream
    }

    /// Whether Bridle's own code exempted this destination from the address check (the self-test site).
    pub fn is_exempt(&self, host: &str, port: u16) -> bool {
        is_exempt(&self.shared, host, port)
    }

    /// Lets a session in: returns its secret and its traffic record. The user name is the session id.
    pub fn register(&self, id: &str) -> (String, Arc<Traffic>) {
        let secret = super::random_hex(16);
        let traffic = Arc::new(Traffic::default());
        self.shared
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                secret.clone(),
                Arc::new(Entry {
                    id: id.to_owned(),
                    traffic: Arc::clone(&traffic),
                }),
            );
        (secret, traffic)
    }

    /// Ends a session's access.
    pub fn unregister(&self, id: &str) {
        self.shared
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, entry| entry.id != id);
    }

    /// Stops listening. Connections already open end when their sessions do.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(500));
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.stop();
    }
}

// ----------------------------------------------------------------------------------------------------------- one request

struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn respond(mut stream: &TcpStream, code: u16, reason: &str, body: &str, extra: &[(&str, &str)]) {
    let mut out = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
    // Close our side, then read away what the client is still sending: closing a socket with unread data resets the connection
    // and the client would lose the answer.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let mut sink = [0u8; 4096];
    let mut drained = 0usize;
    while drained < 256 * 1024 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Reads up to the end of the request head. Returns the head and any bytes that followed it.
fn read_head(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut data: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        let n = stream
            .read(&mut buffer)
            .map_err(|e| format!("cannot read the request: {e}"))?;
        if n == 0 {
            return Err("the request ended early".to_owned());
        }
        data.extend_from_slice(&buffer[..n]);
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > MAX_HEAD {
                return Err("the request head is too large".to_owned());
            }
            let rest = data.split_off(end + 4);
            return Ok((data, rest));
        }
        if data.len() > MAX_HEAD {
            return Err("the request head is too large".to_owned());
        }
    }
}

fn parse_head(raw: &[u8]) -> Result<Head, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "the request is not text".to_owned())?;
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("the request line is not METHOD target VERSION".to_owned());
    };
    if !version.starts_with("HTTP/1.")
        || method.is_empty()
        || !method.chars().all(|c| c.is_ascii_uppercase())
        || target.is_empty()
    {
        return Err("not an HTTP/1 request".to_owned());
    }
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("a header line has no colon".to_owned());
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("a header name is not valid".to_owned());
        }
        if headers.len() >= MAX_HEADERS {
            return Err("too many headers".to_owned());
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    Ok(Head {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
    })
}

/// Standard base64 decoding for the proxy credentials.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.trim_end_matches('=').chars() {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(1));
    }
    diff == 0
}

fn authenticate(shared: &Shared, head: &Head) -> Option<Arc<Entry>> {
    let value = head.header("Proxy-Authorization")?;
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = base64_decode(encoded.trim())?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, secret) = text.split_once(':')?;
    let sessions = shared
        .sessions
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut found = None;
    for (token, entry) in sessions.iter() {
        if same(token.as_bytes(), secret.as_bytes()) && same(entry.id.as_bytes(), user.as_bytes()) {
            found = Some(Arc::clone(entry));
        }
    }
    found
}

/// Splits `host:port` or `[v6]:port`.
fn split_authority(authority: &str, default_port: Option<u16>) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or("an IPv6 address is missing its ]")?;
        let port = after.strip_prefix(':').map(str::to_owned);
        (host.to_owned(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        (host.to_owned(), Some(port.to_owned()))
    } else {
        (authority.to_owned(), None)
    };
    let port = match port {
        Some(p) => p
            .parse::<u16>()
            .map_err(|_| "the port is not a number".to_owned())?,
        None => default_port.ok_or("the address has no port")?,
    };
    if host.is_empty()
        || host.contains('@')
        || host.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("the host is not valid".to_owned());
    }
    Ok((host, port))
}

enum Kind {
    Tunnel {
        host: String,
        port: u16,
    },
    Plain {
        host: String,
        port: u16,
        path: String,
    },
}

fn classify(head: &Head) -> Result<Kind, String> {
    if head.method == "CONNECT" {
        let (host, port) = split_authority(&head.target, None)?;
        return Ok(Kind::Tunnel { host, port });
    }
    let rest = head
        .target
        .strip_prefix("http://")
        .ok_or("only http:// requests and CONNECT tunnels go through this proxy")?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_owned()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_owned()),
    };
    let (host, port) = split_authority(authority, Some(80))?;
    Ok(Kind::Plain { host, port, path })
}

fn is_exempt(shared: &Shared, host: &str, port: u16) -> bool {
    let host_lower = host.trim_end_matches('.').to_ascii_lowercase();
    shared
        .config
        .exempt
        .iter()
        .any(|(h, p)| *p == port && h.eq_ignore_ascii_case(&host_lower))
}

/// What a connection may do: the addresses it may be made to (empty when an upstream proxy resolves the name).
fn admit(shared: &Shared, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    if DENIED_PORTS.contains(&port) {
        return Err(format!(
            "port {port} is not a web port; the browser may not use it"
        ));
    }
    let host_lower = host.trim_end_matches('.').to_ascii_lowercase();
    if is_exempt(shared, host, port) {
        return match host_lower.parse::<IpAddr>() {
            Ok(ip) => Ok(vec![ip]),
            Err(_) => shared
                .config
                .resolver
                .resolve(&host_lower, port)
                .map_err(|e| format!("cannot resolve {host}: {e}")),
        };
    }
    let resolver: Option<&dyn Resolver> = if shared.config.upstream.pins_addresses() {
        Some(shared.config.resolver.as_ref())
    } else {
        None
    };
    guard::resolve_pinned(&host_lower, port, resolver)
}

fn handle(shared: &Shared, mut client: TcpStream) {
    let _ = client.set_read_timeout(Some(HEAD_TIMEOUT));
    let _ = client.set_write_timeout(Some(Duration::from_secs(30)));
    if !client.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
        return respond(
            &client,
            403,
            "Forbidden",
            "Only this machine may use Bridle's egress proxy.",
            &[],
        );
    }
    let (raw, extra) = match read_head(&mut client) {
        Ok(x) => x,
        Err(error) => return respond(&client, 400, "Bad Request", &error, &[]),
    };
    let head = match parse_head(&raw) {
        Ok(h) => h,
        Err(error) => return respond(&client, 400, "Bad Request", &error, &[]),
    };
    let Some(entry) = authenticate(shared, &head) else {
        return respond(
            &client,
            407,
            "Proxy Authentication Required",
            "A Bridle browser session must sign in to this proxy.",
            &[("Proxy-Authenticate", "Basic realm=\"bridle\"")],
        );
    };
    let traffic = &entry.traffic;
    traffic.connections.fetch_add(1, Ordering::SeqCst);
    let kind = match classify(&head) {
        Ok(k) => k,
        Err(error) => return respond(&client, 400, "Bad Request", &error, &[]),
    };
    let (host, port) = match &kind {
        Kind::Tunnel { host, port } | Kind::Plain { host, port, .. } => (host.clone(), *port),
    };
    let addresses = match admit(shared, &host, port) {
        Ok(a) => a,
        Err(reason) => {
            traffic.note_blocked(&host, port, &reason);
            return respond(
                &client,
                403,
                "Forbidden",
                &format!("Bridle's egress proxy refused {host}:{port}: {reason}"),
                &[("X-Bridle-Egress", "blocked")],
            );
        }
    };
    traffic.note_host(&host, port);
    // An exempt destination (Bridle's own test site) is on this machine, which an upstream proxy cannot reach: it is dialled directly.
    let via_upstream = !shared.config.upstream.pins_addresses() && !is_exempt(shared, &host, port);
    let remote = match connect(
        shared,
        &host,
        port,
        &addresses,
        matches!(kind, Kind::Tunnel { .. }),
        via_upstream,
    ) {
        Ok(r) => r,
        Err(error) => {
            return respond(
                &client,
                502,
                "Bad Gateway",
                &format!("Bridle's egress proxy could not reach {host}:{port}: {error}"),
                &[],
            );
        }
    };
    let mut remote = remote;
    match kind {
        Kind::Tunnel { .. } => {
            if client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .and_then(|()| client.flush())
                .is_err()
            {
                return;
            }
        }
        Kind::Plain { host, port, path } => {
            let target = if via_upstream {
                head.target.clone()
            } else {
                path
            };
            let mut out = format!("{} {target} HTTP/1.1\r\n", head.method);
            let mut has_host = false;
            for (name, value) in &head.headers {
                let lower = name.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "proxy-authorization" | "proxy-connection" | "connection" | "keep-alive"
                ) {
                    continue;
                }
                has_host |= lower == "host";
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            if !has_host {
                out.push_str(&format!("Host: {host}:{port}\r\n"));
            }
            if via_upstream
                && let Upstream::Http {
                    auth: Some(auth), ..
                } = &shared.config.upstream
            {
                out.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
            }
            out.push_str("Connection: close\r\n\r\n");
            if remote.write_all(out.as_bytes()).is_err() {
                return;
            }
        }
    }
    if !extra.is_empty() {
        let _ = traffic.count(extra.len() as u64);
        if remote.write_all(&extra).is_err() {
            return;
        }
    }
    pump(shared, client, remote, traffic);
}

/// Opens the connection to the destination: at the address that was judged, or through the upstream proxy.
fn connect(
    shared: &Shared,
    host: &str,
    port: u16,
    addresses: &[IpAddr],
    tunnel: bool,
    via_upstream: bool,
) -> Result<TcpStream, String> {
    match &shared.config.upstream {
        Upstream::Direct => direct(shared, addresses, port),
        Upstream::Http { .. } if !via_upstream => direct(shared, addresses, port),
        Upstream::Http { address, auth } => {
            let mut stream = std::net::ToSocketAddrs::to_socket_addrs(address.as_str())
                .map_err(|e| format!("the upstream proxy {address} cannot be found: {e}"))?
                .find_map(|a| shared.config.dialer.dial(a, DIAL_TIMEOUT).ok())
                .ok_or_else(|| format!("the upstream proxy {address} cannot be reached"))?;
            if tunnel {
                let shown = if host.contains(':') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                };
                let mut request = format!("CONNECT {shown} HTTP/1.1\r\nHost: {shown}\r\n");
                if let Some(auth) = auth {
                    request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
                }
                request.push_str("\r\n");
                stream
                    .write_all(request.as_bytes())
                    .map_err(|e| e.to_string())?;
                let _ = stream.set_read_timeout(Some(DIAL_TIMEOUT));
                let (reply, rest) = read_head(&mut stream)?;
                let status = String::from_utf8_lossy(&reply)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split(' ')
                    .nth(1)
                    .and_then(|c| c.parse::<u16>().ok())
                    .unwrap_or(0);
                if !(200..300).contains(&status) || !rest.is_empty() {
                    return Err(format!(
                        "the upstream proxy answered {status} to the tunnel request"
                    ));
                }
            }
            Ok(stream)
        }
    }
}

/// Dials one of the addresses that were judged, in order, and no other.
fn direct(shared: &Shared, addresses: &[IpAddr], port: u16) -> Result<TcpStream, String> {
    let mut last = "no address".to_owned();
    for ip in addresses {
        match shared
            .config
            .dialer
            .dial(SocketAddr::new(*ip, port), DIAL_TIMEOUT)
        {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Copies both ways until one side closes, the connection is idle for too long, the proxy stops, or the call's byte limit is hit.
fn pump(shared: &Shared, client: TcpStream, remote: TcpStream, traffic: &Arc<Traffic>) {
    let _ = client.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = remote.set_read_timeout(Some(Duration::from_secs(1)));
    let (Ok(client_reader), Ok(remote_reader)) = (client.try_clone(), remote.try_clone()) else {
        return;
    };
    let done = Arc::new(AtomicBool::new(false));
    let one_way =
        |mut from: TcpStream, mut to: TcpStream, done: Arc<AtomicBool>, traffic: Arc<Traffic>| {
            let mut buffer = vec![0u8; 32 * 1024];
            let mut idle = Instant::now();
            loop {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                match from.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        idle = Instant::now();
                        if !traffic.count(n as u64) {
                            break;
                        }
                        if to.write_all(&buffer[..n]).is_err() {
                            break;
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if idle.elapsed() > IDLE_LIMIT {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = to.shutdown(Shutdown::Write);
            // A half-closed tunnel may still carry the answer the other way; the other direction ends when its side does. A cut
            // (limit, idle) closes both.
            done.store(true, Ordering::SeqCst);
            let _ = from.shutdown(Shutdown::Read);
        };
    let stop = &shared.stop;
    thread::scope(|scope| {
        let up = scope.spawn({
            let done = Arc::clone(&done);
            let traffic = Arc::clone(traffic);
            move || one_way(client_reader, remote, done, traffic)
        });
        let down = scope.spawn({
            let done = Arc::clone(&done);
            let traffic = Arc::clone(traffic);
            move || one_way(remote_reader, client, done, traffic)
        });
        while !up.is_finished() || !down.is_finished() {
            if stop.load(Ordering::SeqCst) {
                done.store(true, Ordering::SeqCst);
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A resolver with scripted answers and a count of questions.
    struct Script {
        answers: Mutex<Vec<Vec<IpAddr>>>,
        asked: AtomicUsize,
    }
    impl Script {
        fn new(answers: Vec<Vec<IpAddr>>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers),
                asked: AtomicUsize::new(0),
            })
        }
    }
    impl Resolver for Script {
        fn resolve(&self, _: &str, _: u16) -> io::Result<Vec<IpAddr>> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let mut answers = self.answers.lock().unwrap();
            Ok(if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            })
        }
    }

    /// A dialer that sends every connection to one local server and remembers where it was asked to go.
    struct Redirect {
        to: SocketAddr,
        seen: Mutex<Vec<SocketAddr>>,
    }
    impl Dialer for Redirect {
        fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
            self.seen.lock().unwrap().push(address);
            TcpStream::connect_timeout(&self.to, timeout)
        }
    }

    /// A server that records what it receives and answers once with `reply`, or echoes when `reply` is empty.
    fn server(reply: &'static [u8]) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                thread::spawn(move || {
                    let mut stream = stream;
                    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut buffer = [0u8; 4096];
                    while let Ok(n) = stream.read(&mut buffer) {
                        if n == 0 {
                            break;
                        }
                        log.lock().unwrap().extend_from_slice(&buffer[..n]);
                        if reply.is_empty() {
                            let _ = stream.write_all(&buffer[..n]);
                        } else {
                            let _ = stream.write_all(reply);
                            break;
                        }
                    }
                });
            }
        });
        (address, seen)
    }

    struct Rig {
        proxy: EgressProxy,
        id: String,
        secret: String,
        traffic: Arc<Traffic>,
    }

    fn rig(config: ProxyConfig) -> Rig {
        let proxy = EgressProxy::start(config).unwrap();
        let id = "s-test".to_owned();
        let (secret, traffic) = proxy.register(&id);
        traffic.begin_call(10 * 1024 * 1024);
        Rig {
            proxy,
            id,
            secret,
            traffic,
        }
    }

    fn auth(rig: &Rig) -> String {
        format!(
            "Proxy-Authorization: Basic {}\r\n",
            super::super::runtime::base64(format!("{}:{}", rig.id, rig.secret).as_bytes())
        )
    }

    fn ask(rig: &Rig, request: &str) -> String {
        let mut stream = TcpStream::connect(rig.proxy.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(n) = stream.read(&mut buffer) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n]);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn connect_request(rig: &Rig, authority: &str) -> String {
        format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{}\r\n",
            auth(rig)
        )
    }

    fn public() -> IpAddr {
        "93.184.216.34".parse().unwrap()
    }

    #[test]
    fn a_session_must_sign_in_and_a_finished_session_is_signed_out() {
        let rig = rig(ProxyConfig::new(Upstream::Direct));
        let anonymous = ask(
            &rig,
            "CONNECT example.org:443 HTTP/1.1\r\nHost: example.org:443\r\n\r\n",
        );
        assert!(anonymous.starts_with("HTTP/1.1 407"), "{anonymous}");
        assert!(anonymous.contains("Proxy-Authenticate"));
        let wrong = ask(
            &rig,
            &format!(
                "CONNECT example.org:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
                super::super::runtime::base64(format!("{}:not-the-secret", rig.id).as_bytes())
            ),
        );
        assert!(wrong.starts_with("HTTP/1.1 407"), "{wrong}");
        let other_user = ask(
            &rig,
            &format!(
                "CONNECT example.org:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
                super::super::runtime::base64(format!("s-other:{}", rig.secret).as_bytes())
            ),
        );
        assert!(other_user.starts_with("HTTP/1.1 407"), "{other_user}");
        rig.proxy.unregister(&rig.id);
        let after = ask(&rig, &connect_request(&rig, "example.org:443"));
        assert!(after.starts_with("HTTP/1.1 407"), "{after}");
    }

    #[test]
    fn the_inner_network_is_refused_in_every_spelling_and_each_refusal_is_recorded() {
        let resolver = Script::new(vec![vec!["10.0.0.5".parse().unwrap()]]);
        let rig = rig(ProxyConfig {
            resolver: resolver.clone(),
            ..ProxyConfig::new(Upstream::Direct)
        });
        let blocked = [
            "127.0.0.1:80",
            "localhost:80",
            "localhost.:80",
            "LOCALHOST:8080",
            "169.254.169.254:80",
            "[::1]:80",
            "[::ffff:127.0.0.1]:80",
            "[fe80::1]:80",
            "2130706433:80",
            "0x7f.1:80",
            "0177.0.0.1:80",
            "10.1:80",
            "192.168.1.1:443",
            "printer.local:80",
            "metadata.internal:80",
            // A name that resolves (here, to 10.0.0.5) into the inner network.
            "intranet.example.org:443",
        ];
        for authority in blocked {
            let answer = ask(&rig, &connect_request(&rig, authority));
            assert!(answer.starts_with("HTTP/1.1 403"), "{authority}: {answer}");
            assert!(answer.contains("X-Bridle-Egress: blocked"), "{authority}");
        }
        let (count, list) = rig.traffic.blocked();
        assert_eq!(count as usize, blocked.len());
        assert!(
            list.iter().any(|b| b.reason.contains("10.0.0.5")),
            "{list:?}"
        );
        assert!(
            rig.traffic.hosts().is_empty(),
            "a refused host is not a contacted host"
        );
        // Plain HTTP is judged the same way.
        let plain = ask(
            &rig,
            &format!(
                "GET http://127.0.0.1:80/ HTTP/1.1\r\nHost: 127.0.0.1\r\n{}\r\n",
                auth(&rig)
            ),
        );
        assert!(plain.starts_with("HTTP/1.1 403"), "{plain}");
    }

    #[test]
    fn ports_of_other_services_are_refused() {
        let rig = rig(ProxyConfig {
            resolver: Script::new(vec![vec![public()]]),
            ..ProxyConfig::new(Upstream::Direct)
        });
        for port in [22, 25, 445, 3306, 6379] {
            let answer = ask(&rig, &connect_request(&rig, &format!("example.org:{port}")));
            assert!(answer.starts_with("HTTP/1.1 403"), "{port}: {answer}");
        }
    }

    #[test]
    fn a_public_name_is_resolved_once_and_dialled_at_the_address_that_was_checked() {
        let (fixture, _) = server(b"");
        let resolver = Script::new(vec![vec![public()]]);
        let dialer = Arc::new(Redirect {
            to: fixture,
            seen: Mutex::new(Vec::new()),
        });
        let rig = rig(ProxyConfig {
            resolver: resolver.clone(),
            dialer: dialer.clone(),
            ..ProxyConfig::new(Upstream::Direct)
        });
        let mut stream = TcpStream::connect(rig.proxy.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(connect_request(&rig, "example.org:443").as_bytes())
            .unwrap();
        let mut head = [0u8; 39];
        stream.read_exact(&mut head).unwrap();
        assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200 Connection Established"));
        stream.write_all(b"hello through the tunnel").unwrap();
        let mut echoed = [0u8; 24];
        stream.read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, b"hello through the tunnel");
        assert_eq!(
            resolver.asked.load(Ordering::SeqCst),
            1,
            "one lookup, not two"
        );
        assert_eq!(
            dialer.seen.lock().unwrap().as_slice(),
            &[SocketAddr::new(public(), 443)]
        );
        assert_eq!(rig.traffic.hosts(), vec!["example.org:443".to_owned()]);
        assert!(rig.traffic.total_bytes() >= 48);
    }

    #[test]
    fn a_name_that_starts_answering_with_an_inner_address_is_refused_on_the_next_request() {
        // The classic rebinding script: the first answer is public, the later ones are 127.0.0.1.
        let (fixture, _) = server(b"");
        let resolver = Script::new(vec![vec![public()], vec!["127.0.0.1".parse().unwrap()]]);
        let rig = rig(ProxyConfig {
            resolver,
            dialer: Arc::new(Redirect {
                to: fixture,
                seen: Mutex::new(Vec::new()),
            }),
            ..ProxyConfig::new(Upstream::Direct)
        });
        let first = ask_first_line(&rig, "rebind.example.org:443");
        assert!(first.contains("200"), "{first}");
        let second = ask_first_line(&rig, "rebind.example.org:443");
        assert!(second.contains("403"), "{second}");
        // And an answer that mixes a public and an inner address is refused whole.
        let mixed = Script::new(vec![vec![public(), "10.0.0.9".parse().unwrap()]]);
        let rig2 = self::rig(ProxyConfig {
            resolver: mixed,
            ..ProxyConfig::new(Upstream::Direct)
        });
        assert!(ask_first_line(&rig2, "mixed.example.org:443").contains("403"));
    }

    fn ask_first_line(rig: &Rig, authority: &str) -> String {
        let mut stream = TcpStream::connect(rig.proxy.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(connect_request(rig, authority).as_bytes())
            .unwrap();
        let mut out = vec![0u8; 64];
        let n = stream.read(&mut out).unwrap_or(0);
        String::from_utf8_lossy(&out[..n])
            .lines()
            .next()
            .unwrap_or("")
            .to_owned()
    }

    #[test]
    fn plain_http_is_forwarded_in_the_short_form_with_the_proxy_credentials_removed() {
        let (fixture, seen) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
        let rig = rig(ProxyConfig {
            resolver: Script::new(vec![vec![public()]]),
            dialer: Arc::new(Redirect {
                to: fixture,
                seen: Mutex::new(Vec::new()),
            }),
            ..ProxyConfig::new(Upstream::Direct)
        });
        let answer = ask(
            &rig,
            &format!(
                "GET http://example.org/a/b?x=1 HTTP/1.1\r\nHost: example.org\r\nUser-Agent: t\r\nProxy-Connection: keep-alive\r\n{}\r\n",
                auth(&rig)
            ),
        );
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.ends_with("hello"));
        let request = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
        assert!(
            request.starts_with("GET /a/b?x=1 HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Host: example.org\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(
            !request.to_ascii_lowercase().contains("proxy-"),
            "{request}"
        );
    }

    #[test]
    fn an_exempt_server_is_reached_and_nothing_else_on_that_machine_is() {
        let (fixture, seen) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let rig = rig(ProxyConfig {
            exempt: vec![("127.0.0.1".to_owned(), fixture.port())],
            ..ProxyConfig::new(Upstream::Direct)
        });
        let ok = ask(
            &rig,
            &format!(
                "GET http://127.0.0.1:{}/ HTTP/1.1\r\nHost: x\r\n{}\r\n",
                fixture.port(),
                auth(&rig)
            ),
        );
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(!seen.lock().unwrap().is_empty());
        let other = ask(
            &rig,
            &format!(
                "GET http://127.0.0.1:{}/ HTTP/1.1\r\nHost: x\r\n{}\r\n",
                fixture.port() + 1,
                auth(&rig)
            ),
        );
        assert!(other.starts_with("HTTP/1.1 403"), "{other}");
    }

    #[test]
    fn through_an_upstream_proxy_names_are_forwarded_and_numbers_are_still_refused() {
        // A stand-in for the person's proxy: accepts a tunnel request and echoes.
        let (upstream, seen) = server(b"HTTP/1.1 200 Connection established\r\n\r\n");
        let rig = rig(ProxyConfig::new(Upstream::Http {
            address: upstream.to_string(),
            auth: Some("dXNlcjpwdw==".to_owned()),
        }));
        let allowed = ask_first_line(&rig, "example.org:443");
        assert!(allowed.contains("200"), "{allowed}");
        let request = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
        assert!(
            request.starts_with("CONNECT example.org:443 HTTP/1.1"),
            "{request}"
        );
        assert!(
            request.contains("Proxy-Authorization: Basic dXNlcjpwdw=="),
            "{request}"
        );
        let before = seen.lock().unwrap().len();
        for authority in [
            "127.0.0.1:80",
            "localhost:80",
            "2130706433:80",
            "[::1]:80",
            "10.0.0.1:443",
        ] {
            assert!(
                ask_first_line(&rig, authority).contains("403"),
                "{authority}"
            );
        }
        assert_eq!(
            seen.lock().unwrap().len(),
            before,
            "the upstream proxy was not asked about an inner address"
        );
        assert!(!rig.proxy.upstream().pins_addresses());
    }

    #[test]
    fn an_exempt_server_is_dialled_directly_even_behind_an_upstream_proxy() {
        // Bridle's own test site is on this machine, which the person's proxy cannot reach.
        let (upstream, upstream_seen) =
            server(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
        let (fixture, seen) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let rig = rig(ProxyConfig {
            exempt: vec![("127.0.0.1".to_owned(), fixture.port())],
            ..ProxyConfig::new(Upstream::Http {
                address: upstream.to_string(),
                auth: Some("dXNlcjpwdw==".to_owned()),
            })
        });
        let answer = ask(
            &rig,
            &format!(
                "GET http://127.0.0.1:{}/x HTTP/1.1\r\nHost: x\r\n{}\r\n",
                fixture.port(),
                auth(&rig)
            ),
        );
        assert!(
            answer.starts_with("HTTP/1.1 200") && answer.ends_with("ok"),
            "{answer}"
        );
        let request = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
        assert!(request.starts_with("GET /x HTTP/1.1\r\n"), "{request}");
        assert!(
            !request.contains("Proxy-Authorization"),
            "the person's proxy credentials went to the local site: {request}"
        );
        assert!(
            upstream_seen.lock().unwrap().is_empty(),
            "the upstream proxy was used for a local destination"
        );
    }

    #[test]
    fn a_call_that_moves_more_than_its_limit_is_cut() {
        let (fixture, _) = server(b"");
        let rig = rig(ProxyConfig {
            resolver: Script::new(vec![vec![public()]]),
            dialer: Arc::new(Redirect {
                to: fixture,
                seen: Mutex::new(Vec::new()),
            }),
            ..ProxyConfig::new(Upstream::Direct)
        });
        rig.traffic.begin_call(10_000);
        let mut stream = TcpStream::connect(rig.proxy.address()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(connect_request(&rig, "example.org:443").as_bytes())
            .unwrap();
        let mut head = [0u8; 39];
        stream.read_exact(&mut head).unwrap();
        let chunk = vec![7u8; 8_000];
        for _ in 0..8 {
            if stream.write_all(&chunk).is_err() {
                break;
            }
        }
        let mut received = 0usize;
        let mut buffer = [0u8; 8192];
        while let Ok(n) = stream.read(&mut buffer) {
            if n == 0 {
                break;
            }
            received += n;
        }
        assert!(
            received < 64_000,
            "the transfer should have been cut, but {received} bytes came back"
        );
        assert!(rig.traffic.call_bytes() >= 10_000);
    }

    #[test]
    fn a_proxy_url_is_read_into_an_address_and_credentials_and_other_kinds_are_refused() {
        assert_eq!(
            Upstream::parse("http://172.31.2.4:8080").unwrap(),
            Upstream::Http {
                address: "172.31.2.4:8080".to_owned(),
                auth: None
            }
        );
        let with = Upstream::parse("http://user:pw@proxy.example:3128/").unwrap();
        assert_eq!(
            with,
            Upstream::Http {
                address: "proxy.example:3128".to_owned(),
                auth: Some("dXNlcjpwdw==".to_owned())
            }
        );
        assert_eq!(
            Upstream::parse("proxy.example").unwrap(),
            Upstream::Http {
                address: "proxy.example:80".to_owned(),
                auth: None
            }
        );
        assert!(Upstream::parse("socks5://127.0.0.1:1080").is_err());
        assert!(Upstream::parse("https://proxy.example:443").is_err());
        let env = |n: &str| (n == "HTTPS_PROXY").then(|| "http://p:1".to_owned());
        assert!(matches!(
            Upstream::from_route(&RemoteRoute::Environment, &env).unwrap(),
            Upstream::Http { .. }
        ));
        assert_eq!(
            Upstream::from_route(&RemoteRoute::Direct, &env).unwrap(),
            Upstream::Direct
        );
        assert_eq!(
            Upstream::from_route(&RemoteRoute::Environment, &|_| None).unwrap(),
            Upstream::Direct
        );
    }

    #[test]
    fn badly_formed_requests_get_an_error_and_do_not_hurt_the_proxy() {
        let rig = rig(ProxyConfig::new(Upstream::Direct));
        for junk in [
            "\r\n\r\n",
            "GET\r\n\r\n",
            "get / HTTP/1.1\r\n\r\n",
            "CONNECT a:b HTTP/1.1\r\nbroken header\r\n\r\n",
        ] {
            let answer = ask(&rig, junk);
            assert!(
                answer.starts_with("HTTP/1.1 400") || answer.is_empty(),
                "{junk:?}: {answer}"
            );
        }
        let long = format!(
            "GET / HTTP/1.1\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEAD + 100)
        );
        assert!(ask(&rig, &long).starts_with("HTTP/1.1 400"));
        // Still alive.
        assert!(ask(&rig, "CONNECT x:1 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 407"));
    }
}
