//! Which addresses the web tools may contact.
//!
//! A model-driven fetch is the classic way to reach a machine's inner network: a URL for `http://169.254.169.254/` (a cloud
//! metadata service), for a router, for a service on `localhost`. So every URL is checked **before** it is contacted, and so is
//! every redirect destination: only `http` and `https`, no credentials in the address, and a host that is not on this machine
//! or this network (loopback, private, link-local, CGNAT, multicast, unspecified and the IPv6 equivalents, also when written as an
//! IPv4-mapped address).
//!
//! Residual risk, stated plainly: a name is resolved by this check and again by the connection, so a hostile DNS server could answer
//! differently the second time (rebinding). When a proxy carries the request the proxy resolves the name and enforces its own
//! rules, so the names are not resolved here and only literal addresses and obviously local names are refused.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

/// The longest URL accepted.
pub const MAX_URL_LEN: usize = 2_048;

/// A URL the web tools may consider, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// The URL as it will be requested (credentials are refused, so nothing is removed from it).
    pub url: String,
}

/// Parses and checks the form of a URL: `http` or `https`, a host, a sensible port, no credentials, no control characters.
pub fn parse_url(raw: &str) -> Result<Target, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_URL_LEN {
        return Err(format!(
            "a URL must be between 1 and {MAX_URL_LEN} characters"
        ));
    }
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the URL contains a space or a control character".to_owned());
    }
    let uri: ureq::http::Uri = raw.parse().map_err(|e| format!("not a valid URL: {e}"))?;
    let scheme = uri.scheme_str().unwrap_or("").to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "only http and https URLs are allowed, not `{scheme}`"
        ));
    }
    let authority = uri.authority().ok_or("the URL has no host")?;
    if authority.as_str().contains('@') {
        return Err("a URL with a user name or password is refused".to_owned());
    }
    let host = uri
        .host()
        .ok_or("the URL has no host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    // `localhost.` and `127.0.0.1.` name the same machines as without the dot; the dot is only a way around a name check.
    let host = host.trim_end_matches('.').to_owned();
    if host.is_empty() {
        return Err("the URL has no host".to_owned());
    }
    let port = uri
        .port_u16()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    if port == 0 {
        return Err("port 0 is not allowed".to_owned());
    }
    Ok(Target {
        scheme,
        host,
        port,
        url: raw.to_owned(),
    })
}

/// Why an address must not be contacted, or `None` when it is an ordinary public address.
pub fn blocked_ip(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return blocked_v4(mapped);
            }
            let seg = v6.segments();
            if v6.is_loopback() {
                Some("a loopback address")
            } else if v6.is_unspecified() {
                Some("the unspecified address")
            } else if v6.is_multicast() {
                Some("a multicast address")
            } else if seg[0] & 0xfe00 == 0xfc00 {
                Some("a private (unique local) address")
            } else if seg[0] & 0xffc0 == 0xfe80 {
                Some("a link-local address")
            } else if seg[0] == 0x2001 && seg[1] == 0x0db8 {
                Some("a documentation address")
            } else if v6 == Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0) {
                Some("an unusable address")
            } else {
                None
            }
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, c, _] = ip.octets();
    if ip.is_loopback() {
        Some("a loopback address")
    } else if ip.is_unspecified() || a == 0 {
        Some("the unspecified address")
    } else if ip.is_private() {
        Some("a private network address")
    } else if ip.is_link_local() {
        Some("a link-local address (cloud metadata services live here)")
    } else if a == 100 && (64..=127).contains(&b) {
        Some("a carrier-grade NAT address")
    } else if a == 198 && (b == 18 || b == 19) {
        Some("a benchmarking address")
    } else if ip.is_multicast() || a >= 240 {
        Some("a multicast or reserved address")
    } else if (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
    {
        Some("a documentation address")
    } else if ip.is_broadcast() {
        Some("the broadcast address")
    } else {
        None
    }
}

/// Reads a host written the way old resolvers accept an IPv4 address: one to four parts, each decimal, `0x` hexadecimal or
/// leading-zero octal, the last part filling the rest (`2130706433`, `0x7f.1`, `0177.0.0.1`, `127.1`). A browser or a proxy on the
/// other side may read the same text as an address, so the guard must as well. `None` when the text is not such a number.
pub fn parse_loose_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in &parts {
        let (digits, radix) =
            if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                (hex, 16)
            } else if part.len() > 1 && part.starts_with('0') {
                (&part[1..], 8)
            } else {
                (*part, 10)
            };
        if digits.is_empty() && radix == 16 {
            return None;
        }
        numbers.push(u64::from_str_radix(digits, radix).ok()?);
    }
    let (last, init) = numbers.split_last()?;
    if init.iter().any(|n| *n > 255) {
        return None;
    }
    let tail_bits = 8 * (4 - init.len());
    if tail_bits < 32 && *last >= (1u64 << tail_bits) {
        return None;
    }
    if *last > u64::from(u32::MAX) {
        return None;
    }
    let mut value: u64 = *last;
    for (i, n) in init.iter().enumerate() {
        value |= n << (8 * (3 - i));
    }
    Some(Ipv4Addr::from(u32::try_from(value).ok()?))
}

/// The address a host name stands for when it is written as a number rather than a name: a standard IPv4 or IPv6 address, or one
/// of the old spellings of IPv4 (see [`parse_loose_ipv4`]).
fn literal_ip(host: &str) -> Option<IpAddr> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    // A zone (`fe80::1%25eth0`) does not change which network the address is on.
    let bare = bare.split('%').next().unwrap_or(bare);
    bare.parse::<IpAddr>()
        .ok()
        .or_else(|| parse_loose_ipv4(bare).map(IpAddr::V4))
}

/// Checks a host (and its port) that something else asked us to contact, and returns the addresses it may be contacted at.
///
/// This is the check the browser's egress proxy makes for **every** connection the browser opens. A number is judged by what it
/// is; a local name is refused without asking DNS; with a `resolver` the name is resolved **once** and every answer is judged
/// (one blocked answer refuses the host), and the caller must connect to one of the returned addresses and nowhere else, which is
/// what closes DNS rebinding. Without a `resolver` (a proxy carries the connection and resolves the name) the returned list is
/// empty and only the name itself was judged.
pub fn resolve_pinned(
    host: &str,
    port: u16,
    resolver: Option<&dyn Resolver>,
) -> Result<Vec<IpAddr>, String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err("the host is empty".to_owned());
    }
    if port == 0 {
        return Err("port 0 is not allowed".to_owned());
    }
    if let Some(ip) = literal_ip(&host) {
        return match blocked_ip(ip) {
            Some(why) => Err(format!(
                "{host} is {why}; the web tools do not contact addresses on this machine or its network"
            )),
            None => Ok(vec![ip]),
        };
    }
    // No real top-level domain begins with a digit, so a name that ends in one is a number written in a way we cannot read:
    // refuse it rather than hand it on to be read some other way.
    if host
        .rsplit('.')
        .next()
        .and_then(|label| label.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
    {
        return Err(format!("{host} is not a usable address or name"));
    }
    if local_name(&host) {
        return Err(format!(
            "{host} names this machine or a local network; the web tools do not contact it"
        ));
    }
    let Some(resolver) = resolver else {
        return Ok(Vec::new());
    };
    let addresses = resolver
        .resolve(&host, port)
        .map_err(|e| format!("cannot resolve {host}: {e}"))?;
    if addresses.is_empty() {
        return Err(format!("{host} has no address"));
    }
    if let Some((ip, why)) = addresses
        .iter()
        .find_map(|ip| blocked_ip(*ip).map(|why| (ip, why)))
    {
        return Err(format!("{host} resolves to {ip}, which is {why}"));
    }
    Ok(addresses)
}

/// Names that mean "this machine or this network", refused without asking DNS.
fn local_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
        || !host.contains('.') && host.parse::<IpAddr>().is_err()
}

/// Looks up the addresses of a name.
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

/// The system's resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        Ok((host, port).to_socket_addrs()?.map(|a| a.ip()).collect())
    }
}

/// Checks a target before it is contacted. `resolver` is `None` when a proxy resolves names (see the module note). With
/// `allow_private` the check passes anything: that is for the person's own configured providers and for tests, never for a
/// URL the model chose.
pub fn check_target(
    target: &Target,
    resolver: Option<&dyn Resolver>,
    allow_private: bool,
) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    if let Some(ip) = literal_ip(&target.host) {
        return match blocked_ip(ip) {
            Some(why) => Err(format!(
                "{} is {why}; the web tools do not contact addresses on this machine or its network",
                target.host
            )),
            None => Ok(()),
        };
    }
    if local_name(&target.host) {
        return Err(format!(
            "{} names this machine or a local network; the web tools do not contact it",
            target.host
        ));
    }
    if let Some(resolver) = resolver {
        let addresses = resolver
            .resolve(&target.host, target.port)
            .map_err(|e| format!("cannot resolve {}: {e}", target.host))?;
        if addresses.is_empty() {
            return Err(format!("{} has no address", target.host));
        }
        if let Some((ip, why)) = addresses
            .iter()
            .find_map(|ip| blocked_ip(*ip).map(|why| (ip, why)))
        {
            return Err(format!("{} resolves to {ip}, which is {why}", target.host));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct Fixed(Vec<IpAddr>);
    impl Resolver for Fixed {
        fn resolve(&self, _: &str, _: u16) -> std::io::Result<Vec<IpAddr>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn old_spellings_of_an_ipv4_address_are_read_as_the_address_they_are() {
        for (text, ip) in [
            ("2130706433", "127.0.0.1"),
            ("0x7f.1", "127.0.0.1"),
            ("0177.0.0.1", "127.0.0.1"),
            ("127.1", "127.0.0.1"),
            ("0x7f000001", "127.0.0.1"),
            ("169.254.43518", "169.254.169.254"),
            ("10.1", "10.0.0.1"),
        ] {
            assert_eq!(parse_loose_ipv4(text), Some(ip.parse().unwrap()), "{text}");
            assert!(
                resolve_pinned(text, 80, None).is_err(),
                "{text} must be refused"
            );
        }
        // Not numbers in that sense.
        for text in [
            "",
            "1.2.3.4.5",
            "256.1.1.1",
            "1..2",
            "0x",
            "example.com",
            "1.1.1.256",
        ] {
            assert_eq!(parse_loose_ipv4(text), None, "{text}");
        }
        // A public address written the old way is still public.
        assert_eq!(
            resolve_pinned("0x08.0x08.0x08.0x08", 443, None).unwrap(),
            vec!["8.8.8.8".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn a_trailing_dot_does_not_get_a_local_name_through() {
        for host in [
            "localhost.",
            "LOCALHOST",
            "printer.local.",
            "127.0.0.1.",
            "a.b.localhost.",
        ] {
            assert!(
                resolve_pinned(host, 80, None).is_err(),
                "{host} must be refused"
            );
        }
        assert!(parse_url("http://localhost./x").is_ok_and(|t| t.host == "localhost"));
        assert!(check_target(&target("http://localhost./x"), None, false).is_err());
    }

    #[test]
    fn every_form_of_a_local_ipv6_address_is_refused() {
        for host in [
            "[::1]",
            "::1",
            "[::ffff:127.0.0.1]",
            "[::ffff:7f00:1]",
            "[fe80::1]",
            "[fe80::1%25eth0]",
            "[fc00::1]",
            "[fd12:3456::1]",
            "[::]",
            "[::ffff:169.254.169.254]",
        ] {
            assert!(
                resolve_pinned(host, 443, None).is_err(),
                "{host} must be refused"
            );
        }
        assert!(resolve_pinned("[2606:4700:4700::1111]", 443, None).is_ok());
    }

    #[test]
    fn a_name_is_resolved_once_and_one_bad_answer_refuses_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counting(AtomicUsize, Vec<IpAddr>);
        impl Resolver for Counting {
            fn resolve(&self, _: &str, _: u16) -> std::io::Result<Vec<IpAddr>> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(self.1.clone())
            }
        }
        let public: IpAddr = "93.184.216.34".parse().unwrap();
        let ok = Counting(AtomicUsize::new(0), vec![public]);
        assert_eq!(
            resolve_pinned("example.org", 443, Some(&ok)).unwrap(),
            vec![public]
        );
        assert_eq!(ok.0.load(Ordering::SeqCst), 1);
        let mixed = Counting(
            AtomicUsize::new(0),
            vec![public, "10.0.0.5".parse().unwrap()],
        );
        let error = resolve_pinned("rebind.example.org", 443, Some(&mixed)).unwrap_err();
        assert!(error.contains("10.0.0.5"), "{error}");
        let none = Counting(AtomicUsize::new(0), vec![]);
        assert!(resolve_pinned("nowhere.example.org", 443, Some(&none)).is_err());
        // Without a resolver the name is judged and nothing is returned.
        assert!(resolve_pinned("example.org", 443, None).unwrap().is_empty());
        assert!(resolve_pinned("example.org", 0, None).is_err());
    }

    fn target(url: &str) -> Target {
        parse_url(url).unwrap()
    }

    #[test]
    fn only_http_and_https_without_credentials_pass_and_the_default_ports_are_filled_in() {
        assert_eq!(target("https://example.org/a?b=1").port, 443);
        assert_eq!(target("http://example.org:8080/").port, 8080);
        for bad in [
            "ftp://example.org/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pw@example.org/",
            "https://example.org/a b",
            "",
            "https:///nohost",
            "http://example.org:0/",
        ] {
            assert!(parse_url(bad).is_err(), "{bad} must be refused");
        }
        assert!(parse_url(&format!("https://example.org/{}", "a".repeat(MAX_URL_LEN))).is_err());
    }

    #[test]
    fn addresses_on_this_machine_or_network_are_refused_in_every_spelling() {
        for ip in [
            "127.0.0.1",
            "127.8.8.8",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd12:3456::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(
                blocked_ip(ip.parse().unwrap()).is_some(),
                "{ip} must be blocked"
            );
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "172.32.0.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(
                blocked_ip(ip.parse().unwrap()).is_none(),
                "{ip} must be allowed"
            );
        }
    }

    #[test]
    fn a_literal_address_and_a_local_name_are_refused_without_dns() {
        for url in [
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost/",
            "http://printer.local/",
            "http://db.internal/",
            "http://router/",
            "http://[::ffff:192.168.0.1]/",
        ] {
            assert!(
                check_target(&target(url), None, false).is_err(),
                "{url} must be refused"
            );
        }
        assert!(
            check_target(&target("https://example.org/"), None, false).is_ok(),
            "an ordinary name passes when a proxy resolves it"
        );
        assert!(check_target(&target("https://8.8.8.8/"), None, false).is_ok());
    }

    #[test]
    fn a_name_that_resolves_to_a_private_address_is_refused_when_this_machine_resolves_it() {
        let rebinding = Fixed(vec![
            "93.184.216.34".parse().unwrap(),
            "10.0.0.5".parse().unwrap(),
        ]);
        let error = check_target(
            &target("https://innocent.example/"),
            Some(&rebinding),
            false,
        )
        .unwrap_err();
        assert!(error.contains("10.0.0.5"), "{error}");
        let fine = Fixed(vec!["93.184.216.34".parse().unwrap()]);
        assert!(check_target(&target("https://innocent.example/"), Some(&fine), false).is_ok());
        assert!(
            check_target(
                &target("https://innocent.example/"),
                Some(&Fixed(vec![])),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn the_persons_own_providers_may_be_on_this_machine() {
        assert!(check_target(&target("http://127.0.0.1:8080/search"), None, true).is_ok());
    }
}
