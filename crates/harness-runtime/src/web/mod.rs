//! Web research (ADR-0019): search providers, fetching pages, downloading files, and the records that say where every piece of
//! information came from. Everything here is policy-gated by the tools in `mcp::tools::web`; this module has no policy of its
//! own beyond the address guard, which refuses to contact the machine's own network.

pub mod download;
pub mod fetch;
pub mod fusion;
pub mod guard;
pub mod html;
pub mod providers;
pub mod store;

/// The `User-Agent` of every web request.
pub const USER_AGENT: &str = "Bridle-web/0.1 (+https://github.com/kushagrathisside/bridle)";
