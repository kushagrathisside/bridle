//! HTTP clients for the model servers (loopback) and for the optional Jev service (remote).
//!
//! `local_agent` never uses a proxy: a proxy would intercept localhost traffic. `remote_agent` uses the proxy the
//! environment names (`HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`), as every other program on the machine does, and checks
//! certificates with the system store through native-tls (SChannel on Windows, OpenSSL elsewhere), so a corporate
//! root certificate in the store is trusted. The config names the endpoint; the environment names the route. See
//! ADR-0007.

use std::time::Duration;

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

/// Builds an agent for a remote endpoint (M7): the environment's proxy, the system certificate store, the given
/// whole-request timeout, and non-2xx statuses returned rather than raised.
pub fn remote_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_agents_are_built_and_can_be_cloned() {
        // The agents' configs are not exposed for inspection; the behavioural checks are the loopback
        // health runs and the live TLS check of the Jev engine.
        let local = local_agent(Duration::from_secs(1));
        let _local_again = local.clone();
        let remote = remote_agent(Duration::from_secs(1));
        let _remote_again = remote.clone();
    }
}
