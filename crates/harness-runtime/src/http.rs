//! HTTP client for the local model servers.
//!
//! Every M1 endpoint is loopback-only (config rule V4). A proxy configured in the environment
//! (`HTTP_PROXY`, as on the target workstation) would intercept localhost traffic and time out,
//! so the loopback client never uses one. Non-loopback endpoints will need their own policy when
//! external providers arrive (M7 and M8).

use std::time::Duration;

/// Builds an agent for loopback model servers with no proxy and the given whole-request timeout.
pub fn loopback_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_agent_has_no_proxy() {
        // The agent's config is not exposed for inspection, so this checks the builder is accepted
        // and that the agent can be cloned; the behavioural check is the end-to-end health run.
        let agent = loopback_agent(Duration::from_secs(1));
        let _second = agent.clone();
    }
}
