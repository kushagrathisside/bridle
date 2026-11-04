//! The managed browser (ADR-0020): Chromium, driven by Microsoft Playwright inside a small worker that Bridle starts, supervises and
//! stops. Nothing in this module decides what the browser may reach; the policy gate in the MCP tools does, and the egress proxy
//! judges every connection the browser makes.

pub mod manager;
pub mod protocol;
pub mod proxy;
pub mod runtime;
pub mod selftest;

/// A random lower-case hexadecimal string of `bytes` bytes. For session ids and proxy secrets, which must not be guessable by a
/// page or by another program on the machine. Each 64 bits come from a fresh `RandomState`, whose SipHash keys the standard
/// library takes from the operating system's random source, mixed with the clock and a counter.
pub(crate) fn random_hex(bytes: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::with_capacity(bytes * 2);
    while out.len() < bytes * 2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::SeqCst));
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        hasher.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.truncate(bytes * 2);
    out
}

#[cfg(test)]
mod tests {
    use super::random_hex;

    #[test]
    fn random_ids_have_the_asked_length_and_do_not_repeat() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..500 {
            let id = random_hex(16);
            assert_eq!(id.len(), 32);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(seen.insert(id));
        }
        assert_eq!(random_hex(5).len(), 10);
    }
}
