//! Semantic validation of a loaded [`Config`]. Parsing guarantees shape; this guarantees meaning.
//!
//! IMPLEMENTATION SPEC (delegated). `validate_with` returns every issue found (never stops at the
//! first). `path` uses dotted TOML paths, e.g. `models.main.endpoint`. Rules:
//!
//! | # | Severity | Rule |
//! |---|---|---|
//! | V1 | Error | `models` is empty |
//! | V2 | Error | `agent.default_model` is not a key of `models` |
//! | V3 | Error | an endpoint does not start with `http://` or `https://`, or has an empty host |
//! | V4 | Error | an endpoint host is not loopback (`127.0.0.0/8`, `localhost`, `[::1]`) unless `external.cloud_models.enabled` and the scheme is `https`; or `capabilities.local == false` while `external.cloud_models.enabled == false` (local-first, R-27.4) |
//! | V5 | Error | `context == 0`, or `max_output_tokens >= context` |
//! | V6 | Error | `launch.runtime` is not a key of `runtimes` |
//! | V7 | Error | provider `prism_bonsai` with a launch runtime whose kind is not `prism_llama_cpp`; or a ternary quantisation (`PTQ1_0`/`PQ2_0` in `source.quantization` or in the `launch.model_file` name, case-insensitive) launched with a `llama_cpp` runtime |
//! | V8 | Error | `api_key_env` (models and `external.jev`) does not match `^[A-Z_][A-Z0-9_]*$` (it probably contains a secret) |
//! | V9 | Error | `context.target_tokens > context.hard_limit` |
//! | V10 | Error | `routing.escalation_threshold` outside `[0, 1]` |
//! | V11 | Error | `decisions.provider` or `decisions.fallback` is `jev` while `external.jev.enabled == false` |
//! | V12 | Error | `external.jev.enabled` or `external.cloud_models.enabled` while `profile == "local-only"` (R-36.2) |
//! | V13 | Error | `sampling.temperature` outside `[0, 2]`, `top_p` outside `(0, 1]`, `min_p` outside `[0, 1]` |
//! | V14 | Warning | `kv_cache` not one of `f16, bf16, q8_0, q4_0, q4_1, q5_0, q5_1, iq4_nl` |
//! | V15 | Warning | `verification.enabled && required_for_done && steps.is_empty()` — DONE can never be verified |
//! | V16 | Error | a launch `model_file` is relative and `paths.models_dir` is `None` (same for runtime `server_bin` / `paths.runtimes_dir`) |
//! | V17 | Warning | two model slots share the same endpoint |
//! | V18 | Warning | `paths.models_dir` is on the system drive (`env.system_drive`, compare case-insensitively on the drive prefix such as `C:`) — directive D-2 |
//! | V19 | Warning | `decisions.fallback != rules` — the fallback should be deterministic (R-30) |
//! | V20 | Error | provider `mock` in any profile other than `test` |
//! | V21 | Error | `context.hard_limit > models[agent.default_model].context` (skipped when the default model is missing) |
//! | V22 | Error | `launch.extra_args` contains a reserved flag ([`super::RESERVED_LAUNCH_FLAGS`]: listener, model, alias, context, key, served or written paths), as `--flag` or `--flag=value` |
//! | V23 | Error | `timeout_ms` outside `1_000..=3_600_000`, or `retries > 10` |
//! | V24 | Warning | a verification step sets `targets = true` but its `kind` is not `test` |
//!
//! Unit tests: one passing baseline config (no issues) and at least one failing case per rule.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::net::IpAddr;
use std::path::Path;

use serde::Serialize;

use super::schema::{
    Config, DecisionProviderKind, LaunchConfig, ModelConfig, ProfileKind, ProviderKind,
    RuntimeConfig, RuntimeKind, StepKind,
};
use super::{is_absolute_any_platform, reserved_launch_flag};

/// How serious a finding is. Errors make the config unusable; warnings are advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The config works, but something is probably not what the user meant.
    Warning,
    /// The config is wrong and must be fixed before use.
    Error,
}

/// One finding from validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigIssue {
    /// Whether the finding blocks use of the config or is advisory.
    pub severity: Severity,
    /// Rule id from the table above, e.g. `V4`.
    pub rule: &'static str,
    /// Dotted TOML path of the offending value.
    pub path: String,
    /// Human-readable explanation. Never echoes secret values.
    pub message: String,
}

/// Facts about the machine that validation needs, injected so validation stays pure and testable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationEnv {
    /// System drive prefix on Windows, e.g. `Some("C:")`. `None` on other platforms.
    pub system_drive: Option<String>,
}

impl ValidationEnv {
    /// Reads `%SystemDrive%` on Windows; empty elsewhere.
    pub fn from_process() -> Self {
        if cfg!(windows) {
            Self {
                system_drive: std::env::var("SystemDrive")
                    .ok()
                    .or_else(|| Some("C:".into())),
            }
        } else {
            Self::default()
        }
    }
}

/// Validates with the current process environment.
pub fn validate(cfg: &Config) -> Vec<ConfigIssue> {
    validate_with(cfg, &ValidationEnv::from_process())
}

/// Runs every rule (V1 to V24) and returns all issues found, in rule order.
pub fn validate_with(cfg: &Config, env: &ValidationEnv) -> Vec<ConfigIssue> {
    let mut report = Report::default();

    check_model_slots(cfg, &mut report);
    for (slot, model) in &cfg.models {
        check_model(cfg, slot, model, &mut report);
    }
    check_shared_endpoints(cfg, &mut report);
    check_runtimes(cfg, &mut report);
    check_profile(cfg, &mut report);
    check_jev(cfg, &mut report);
    check_decisions(cfg, &mut report);
    check_routing(cfg, &mut report);
    check_context(cfg, &mut report);
    check_hard_limit_against_default_model(cfg, &mut report);
    check_verification(cfg, &mut report);
    check_paths(cfg, env, &mut report);

    report.0
}

/// Collects issues while the rule checks run.
#[derive(Default)]
struct Report(Vec<ConfigIssue>);

impl Report {
    fn error(&mut self, rule: &'static str, path: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Error, rule, path, message);
    }

    fn warning(&mut self, rule: &'static str, path: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Warning, rule, path, message);
    }

    fn push(
        &mut self,
        severity: Severity,
        rule: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.0.push(ConfigIssue {
            severity,
            rule,
            path: path.into(),
            message: message.into(),
        });
    }
}

/// Builds a dotted path such as `models.main.endpoint`.
fn model_path(slot: &str, field: &str) -> String {
    format!("models.{slot}.{field}")
}

/// V1 and V2: at least one model slot exists, and the default slot is one of them.
fn check_model_slots(cfg: &Config, report: &mut Report) {
    if cfg.models.is_empty() {
        report.error("V1", "models", "no model slots are configured");
    }
    if !cfg.models.contains_key(&cfg.agent.default_model) {
        report.error(
            "V2",
            "agent.default_model",
            format!("`{}` is not a key of [models]", cfg.agent.default_model),
        );
    }
}

/// Rules that apply to a single model slot.
fn check_model(cfg: &Config, slot: &str, model: &ModelConfig, report: &mut Report) {
    check_endpoint(cfg, slot, model, report);
    check_token_budget(slot, model, report);
    check_sampling(slot, model, report);
    check_kv_cache(slot, model, report);
    check_request_limits(slot, model, report);
    check_api_key_env(
        model_path(slot, "api_key_env"),
        model.api_key_env.as_deref(),
        report,
    );
    if model.provider == ProviderKind::Mock && cfg.profile != ProfileKind::Test {
        report.error(
            "V20",
            model_path(slot, "provider"),
            format!(
                "provider `mock` is only allowed in the `test` profile, not `{}`",
                cfg.profile.as_str()
            ),
        );
    }
    if let Some(launch) = &model.launch {
        check_launch(cfg, slot, model, launch, report);
    }
}

/// V3 and V4: the endpoint is an http(s) URL with a host. A non-loopback endpoint is only allowed
/// when the cloud opt-in is enabled and the scheme is `https`; a non-local model is only allowed
/// when the cloud opt-in is enabled.
fn check_endpoint(cfg: &Config, slot: &str, model: &ModelConfig, report: &mut Report) {
    let host = endpoint_host(&model.endpoint).filter(|host| !host.is_empty());
    if host.is_none() {
        report.error(
            "V3",
            model_path(slot, "endpoint"),
            "endpoint must be an http:// or https:// URL with a non-empty host",
        );
    }

    let cloud_enabled = cfg.external.cloud_models.enabled;
    if let Some(host) = host.filter(|host| !is_loopback_host(host))
        && !(cloud_enabled && is_https(&model.endpoint))
    {
        let hint = if cloud_enabled {
            "a non-loopback endpoint must use https:// when external.cloud_models is enabled"
        } else {
            "set external.cloud_models.enabled and use https:// to reach a non-loopback host"
        };
        report.error(
            "V4",
            model_path(slot, "endpoint"),
            format!("host `{host}` is not loopback; {hint}"),
        );
    }
    if cloud_enabled {
        return;
    }
    if !model.capabilities.local {
        report.error(
            "V4",
            model_path(slot, "capabilities.local"),
            "model is marked non-local; set external.cloud_models.enabled to use it",
        );
    }
}

/// V5: the context window is non-zero and leaves room for the output budget.
fn check_token_budget(slot: &str, model: &ModelConfig, report: &mut Report) {
    if model.context == 0 {
        report.error(
            "V5",
            model_path(slot, "context"),
            "context must be greater than zero",
        );
    } else if model.max_output_tokens >= model.context {
        report.error(
            "V5",
            model_path(slot, "max_output_tokens"),
            format!(
                "max_output_tokens ({}) must be smaller than context ({})",
                model.max_output_tokens, model.context
            ),
        );
    }
}

/// V13: sampling parameters are inside their documented ranges. NaN is out of range everywhere.
fn check_sampling(slot: &str, model: &ModelConfig, report: &mut Report) {
    let sampling = &model.sampling;
    if sampling
        .temperature
        .is_some_and(|t| !(0.0..=2.0).contains(&t))
    {
        report.error(
            "V13",
            model_path(slot, "sampling.temperature"),
            "temperature must be within [0, 2]",
        );
    }
    if sampling.top_p.is_some_and(|p| !(p > 0.0 && p <= 1.0)) {
        report.error(
            "V13",
            model_path(slot, "sampling.top_p"),
            "top_p must be within (0, 1]",
        );
    }
    if sampling.min_p.is_some_and(|p| !(0.0..=1.0).contains(&p)) {
        report.error(
            "V13",
            model_path(slot, "sampling.min_p"),
            "min_p must be within [0, 1]",
        );
    }
}

/// Cache types known to the llama.cpp server for `--cache-type-k` / `--cache-type-v`.
/// V23 bounds for one model request: at least a second, at most an hour, at most ten retries.
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 3_600_000;
const MAX_RETRIES: u32 = 10;

const KV_CACHE_TYPES: [&str; 8] = [
    "f16", "bf16", "q8_0", "q4_0", "q4_1", "q5_0", "q5_1", "iq4_nl",
];

/// V14: an unknown KV-cache type is probably a typo.
fn check_kv_cache(slot: &str, model: &ModelConfig, report: &mut Report) {
    if let Some(kv) = model
        .kv_cache
        .as_deref()
        .filter(|kv| !KV_CACHE_TYPES.contains(kv))
    {
        report.warning(
            "V14",
            model_path(slot, "kv_cache"),
            format!("kv_cache `{kv}` is not a known cache type"),
        );
    }
}

/// V8: `api_key_env` names an environment variable. A value that does not look like one is
/// probably the key itself, so the message deliberately does not echo it.
fn check_api_key_env(path: String, value: Option<&str>, report: &mut Report) {
    if value.is_some_and(|name| !is_env_var_name(name)) {
        report.error(
            "V8",
            path,
            "api_key_env must be the name of an environment variable (e.g. OPENAI_API_KEY), not the key itself",
        );
    }
}

/// V6, V7 and V16 for a model's launch section.
fn check_launch(
    cfg: &Config,
    slot: &str,
    model: &ModelConfig,
    launch: &LaunchConfig,
    report: &mut Report,
) {
    match cfg.runtimes.get(&launch.runtime) {
        None => report.error(
            "V6",
            model_path(slot, "launch.runtime"),
            format!("runtime `{}` is not defined in [runtimes]", launch.runtime),
        ),
        Some(runtime) => check_runtime_fit(slot, model, launch, runtime, report),
    }

    if !is_absolute_any_platform(&launch.model_file) && cfg.paths.models_dir.is_none() {
        report.error(
            "V16",
            model_path(slot, "launch.model_file"),
            "model_file is relative but paths.models_dir is not set",
        );
    }

    if let Some(flag) = reserved_launch_flag(&launch.extra_args) {
        report.error(
            "V22",
            model_path(slot, "launch.extra_args"),
            format!(
                "`{flag}` is set by the launcher or changes what the server listens on, serves or \
                 writes; it cannot be passed in extra_args"
            ),
        );
    }
}

/// V23: request timeouts and retries stay in a sane range, so a typo cannot hang the agent for
/// days or retry forever.
fn check_request_limits(slot: &str, model: &ModelConfig, report: &mut Report) {
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&model.timeout_ms) {
        report.error(
            "V23",
            model_path(slot, "timeout_ms"),
            format!(
                "timeout_ms {} is outside {MIN_TIMEOUT_MS}..={MAX_TIMEOUT_MS}",
                model.timeout_ms
            ),
        );
    }
    if model.retries > MAX_RETRIES {
        report.error(
            "V23",
            model_path(slot, "retries"),
            format!("retries {} is above {MAX_RETRIES}", model.retries),
        );
    }
}

/// V7: `prism_bonsai` models need the PrismML runtime, and ternary weights cannot run on
/// upstream llama.cpp.
fn check_runtime_fit(
    slot: &str,
    model: &ModelConfig,
    launch: &LaunchConfig,
    runtime: &RuntimeConfig,
    report: &mut Report,
) {
    if model.provider == ProviderKind::PrismBonsai && runtime.kind != RuntimeKind::PrismLlamaCpp {
        report.error(
            "V7",
            model_path(slot, "launch.runtime"),
            format!(
                "provider prism_bonsai needs a runtime of kind prism_llama_cpp, but `{}` is not",
                launch.runtime
            ),
        );
    }

    let file = launch.model_file.to_string_lossy();
    let file_name = file.rsplit(['/', '\\']).next().unwrap_or_default();
    let ternary = model
        .source
        .as_ref()
        .is_some_and(|source| is_ternary(&source.quantization))
        || is_ternary(file_name);
    if ternary && runtime.kind == RuntimeKind::LlamaCpp {
        report.error(
            "V7",
            model_path(slot, "launch.runtime"),
            "ternary (PTQ1_0 / PQ2_0) weights need a prism_llama_cpp runtime, not llama_cpp",
        );
    }
}

/// Ternary Bonsai quantisations, matched case-insensitively.
fn is_ternary(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    upper.contains("PTQ1_0") || upper.contains("PQ2_0")
}

/// V17: two slots pointing at the same endpoint. Each later slot in key order is reported.
fn check_shared_endpoints(cfg: &Config, report: &mut Report) {
    let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
    for (slot, model) in &cfg.models {
        match owners.entry(model.endpoint.trim_end_matches('/')) {
            Entry::Occupied(first) => report.warning(
                "V17",
                model_path(slot, "endpoint"),
                format!("endpoint is shared with models.{}", first.get()),
            ),
            Entry::Vacant(owner) => {
                owner.insert(slot.as_str());
            }
        }
    }
}

/// V16 for runtime binaries: a relative `server_bin` needs `paths.runtimes_dir`.
fn check_runtimes(cfg: &Config, report: &mut Report) {
    for (name, runtime) in &cfg.runtimes {
        if !is_absolute_any_platform(&runtime.server_bin) && cfg.paths.runtimes_dir.is_none() {
            report.error(
                "V16",
                format!("runtimes.{name}.server_bin"),
                "server_bin is relative but paths.runtimes_dir is not set",
            );
        }
    }
}

/// V12: the local-only profile forbids every external opt-in (R-36.2).
fn check_profile(cfg: &Config, report: &mut Report) {
    if cfg.profile != ProfileKind::LocalOnly {
        return;
    }
    if cfg.external.jev.enabled {
        report.error(
            "V12",
            "external.jev.enabled",
            "external.jev cannot be enabled in the local-only profile",
        );
    }
    if cfg.external.cloud_models.enabled {
        report.error(
            "V12",
            "external.cloud_models.enabled",
            "external.cloud_models cannot be enabled in the local-only profile",
        );
    }
}

/// V8 for the jev credential name.
fn check_jev(cfg: &Config, report: &mut Report) {
    check_api_key_env(
        "external.jev.api_key_env".to_owned(),
        cfg.external.jev.api_key_env.as_deref(),
        report,
    );
}

/// V11 and V19: `jev` needs its opt-in, and the fallback should be deterministic.
fn check_decisions(cfg: &Config, report: &mut Report) {
    if !cfg.external.jev.enabled {
        if cfg.decisions.provider == DecisionProviderKind::Jev {
            report.error(
                "V11",
                "decisions.provider",
                "decisions.provider is jev but external.jev is not enabled",
            );
        }
        if cfg.decisions.fallback == DecisionProviderKind::Jev {
            report.error(
                "V11",
                "decisions.fallback",
                "decisions.fallback is jev but external.jev is not enabled",
            );
        }
    }
    if cfg.decisions.fallback != DecisionProviderKind::Rules {
        report.warning(
            "V19",
            "decisions.fallback",
            "the fallback should be the deterministic `rules` engine (R-30)",
        );
    }
}

/// V10: the escalation threshold is a confidence in `[0, 1]`.
fn check_routing(cfg: &Config, report: &mut Report) {
    if !(0.0..=1.0).contains(&cfg.routing.escalation_threshold) {
        report.error(
            "V10",
            "routing.escalation_threshold",
            "escalation_threshold must be within [0, 1]",
        );
    }
}

/// V9: the target context must not exceed the hard limit.
fn check_context(cfg: &Config, report: &mut Report) {
    if cfg.context.target_tokens > cfg.context.hard_limit {
        report.error(
            "V9",
            "context.target_tokens",
            format!(
                "target_tokens ({}) exceeds hard_limit ({})",
                cfg.context.target_tokens, cfg.context.hard_limit
            ),
        );
    }
}

/// V21: the hard context limit cannot exceed the window of the default model. Skipped when the
/// default model is missing (V2 reports that).
fn check_hard_limit_against_default_model(cfg: &Config, report: &mut Report) {
    let Some(model) = cfg.models.get(&cfg.agent.default_model) else {
        return;
    };
    if cfg.context.hard_limit > model.context {
        report.error(
            "V21",
            "context.hard_limit",
            format!(
                "hard_limit ({}) exceeds the context ({}) of the default model `{}`",
                cfg.context.hard_limit, model.context, cfg.agent.default_model
            ),
        );
    }
}

/// V15: verification that is required for DONE but has no steps can never succeed.
fn check_verification(cfg: &Config, report: &mut Report) {
    let verification = &cfg.verification;
    if verification.enabled && verification.required_for_done && verification.steps.is_empty() {
        report.warning(
            "V15",
            "verification.steps",
            "verification is required for DONE but no steps are defined",
        );
    }
    for (index, step) in verification.steps.iter().enumerate() {
        if step.targets && step.kind != StepKind::Test {
            report.warning(
                "V24",
                format!("verification.steps[{index}].targets"),
                format!(
                    "targets are only passed by run_tests, but this step's kind is `{}`",
                    step.kind.as_str()
                ),
            );
        }
    }
}

/// V18: the models directory should not sit on the system drive (D-2).
fn check_paths(cfg: &Config, env: &ValidationEnv, report: &mut Report) {
    if let Some(system_drive) = env.system_drive.as_deref()
        && let Some(models_dir) = cfg.paths.models_dir.as_deref()
        && on_drive(models_dir, system_drive)
    {
        report.warning(
            "V18",
            "paths.models_dir",
            "models_dir is on the system drive; keep weights on a data drive (D-2)",
        );
    }
}

/// Whether `path` starts with the drive prefix `drive` (such as `C:`), compared case-insensitively.
fn on_drive(path: &Path, drive: &str) -> bool {
    let drive = drive.trim_end_matches(['\\', '/']);
    let text = path.to_string_lossy();
    !drive.is_empty()
        && text
            .get(..drive.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(drive))
}

/// Host part of an `http://` or `https://` endpoint, without userinfo, port or path. Returns an
/// empty host when the authority has none, and `None` when the scheme is not http(s) or a
/// bracketed IPv6 host is unterminated.
pub(super) fn endpoint_host(endpoint: &str) -> Option<&str> {
    let (scheme, rest) = endpoint.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_port)| host_port);

    if host_port.starts_with('[') {
        // Bracketed IPv6 literal: `[addr]` or `[addr]:port`. The brackets stay in the result.
        let close = host_port.find(']')?;
        return Some(&host_port[..=close]);
    }
    Some(
        host_port
            .split_once(':')
            .map_or(host_port, |(host, _)| host),
    )
}

/// True when the endpoint's scheme is `https` (case-insensitive).
fn is_https(endpoint: &str) -> bool {
    endpoint
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
}

/// Loopback test for an endpoint host: `localhost`, any `127.0.0.0/8` address, or `::1`.
pub(super) fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let address = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    address.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Matches `^[A-Z_][A-Z0-9_]*$`.
fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('A'..='Z' | '_'))
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::from_toml_str;

    const BASELINE: &str = r#"
        profile = "local-only"

        [agent]
        default_model = "main"

        [paths]
        models_dir = "/data/models"
        runtimes_dir = "/data/runtimes"

        [runtimes.llama]
        kind = "llama_cpp"
        server_bin = "llama-server"

        [models.main]
        provider = "llama_cpp"
        id = "qwen3.5-9b"
        role = "engineer"
        endpoint = "http://127.0.0.1:8001/v1"
        context = 16384
        hardware = "gpu"
        kv_cache = "q8_0"

        [models.main.launch]
        runtime = "llama"
        model_file = "qwen3.5-9b-q4_k_m.gguf"

        [models.main.source]
        repo = "unsloth/Qwen3.5-9B-GGUF"
        revision = "0123456789abcdef"
        file = "qwen3.5-9b-q4_k_m.gguf"
        quantization = "Q4_K_M"

        [context]
        target_tokens = 16000
        hard_limit = 16384

        [verification]
        enabled = true
        required_for_done = true

        [[verification.steps]]
        command = "cargo test"
    "#;

    fn baseline() -> Config {
        from_toml_str(BASELINE).expect("baseline parses")
    }

    fn main_model(cfg: &mut Config) -> &mut ModelConfig {
        cfg.models.get_mut("main").expect("main slot")
    }

    fn issues(cfg: &Config) -> Vec<ConfigIssue> {
        validate_with(cfg, &ValidationEnv::default())
    }

    fn assert_issue(cfg: &Config, rule: &str, severity: Severity, path: &str) {
        let found = issues(cfg);
        assert!(
            found
                .iter()
                .any(|i| i.rule == rule && i.severity == severity && i.path == path),
            "expected {rule} {severity:?} at {path}, got {found:#?}"
        );
    }

    fn assert_no_rule(cfg: &Config, rule: &str) {
        let found = issues(cfg);
        assert!(
            found.iter().all(|i| i.rule != rule),
            "expected no {rule}, got {found:#?}"
        );
    }

    #[test]
    fn baseline_has_no_issues() {
        let found = issues(&baseline());
        assert!(found.is_empty(), "unexpected issues: {found:#?}");
    }

    #[test]
    fn every_issue_is_reported_not_just_the_first() {
        let mut cfg = baseline();
        cfg.agent.default_model = "deep".to_owned();
        cfg.context.target_tokens = 30_000;
        assert_issue(&cfg, "V2", Severity::Error, "agent.default_model");
        assert_issue(&cfg, "V9", Severity::Error, "context.target_tokens");
    }

    #[test]
    fn v1_empty_models() {
        let mut cfg = baseline();
        cfg.models.clear();
        assert_issue(&cfg, "V1", Severity::Error, "models");
    }

    #[test]
    fn v2_default_model_must_exist() {
        let mut cfg = baseline();
        cfg.agent.default_model = "deep".to_owned();
        assert_issue(&cfg, "V2", Severity::Error, "agent.default_model");
    }

    #[test]
    fn v3_endpoint_needs_http_scheme_and_host() {
        for endpoint in [
            "127.0.0.1:8001/v1",
            "ftp://127.0.0.1/v1",
            "http://",
            "http://:8001/v1",
            "http://[::1/v1",
        ] {
            let mut cfg = baseline();
            main_model(&mut cfg).endpoint = endpoint.to_owned();
            assert_issue(&cfg, "V3", Severity::Error, "models.main.endpoint");
        }
    }

    #[test]
    fn v4_remote_endpoint_needs_cloud_opt_in() {
        let mut cfg = baseline();
        main_model(&mut cfg).endpoint = "http://10.0.0.5/v1".to_owned();
        assert_issue(&cfg, "V4", Severity::Error, "models.main.endpoint");
    }

    #[test]
    fn v4_remote_https_endpoint_needs_cloud_opt_in() {
        let mut cfg = baseline();
        main_model(&mut cfg).endpoint = "https://api.example.com/v1".to_owned();
        assert_issue(&cfg, "V4", Severity::Error, "models.main.endpoint");

        cfg.profile = ProfileKind::Development;
        cfg.external.cloud_models.enabled = true;
        assert_no_rule(&cfg, "V4");
    }

    #[test]
    fn v4_remote_endpoint_over_plain_http_is_rejected_even_with_cloud() {
        let mut cfg = baseline();
        cfg.profile = ProfileKind::Development;
        cfg.external.cloud_models.enabled = true;
        main_model(&mut cfg).endpoint = "http://10.0.0.5/v1".to_owned();
        assert_issue(&cfg, "V4", Severity::Error, "models.main.endpoint");
    }

    #[test]
    fn v4_non_local_model_needs_cloud_opt_in() {
        let mut cfg = baseline();
        main_model(&mut cfg).capabilities.local = false;
        assert_issue(
            &cfg,
            "V4",
            Severity::Error,
            "models.main.capabilities.local",
        );

        cfg.profile = ProfileKind::Development;
        cfg.external.cloud_models.enabled = true;
        assert_no_rule(&cfg, "V4");
    }

    #[test]
    fn v4_loopback_hosts_are_accepted() {
        for endpoint in [
            "http://127.0.0.1:8001/v1",
            "http://localhost:8001/v1",
            "HTTP://LOCALHOST/v1",
            "http://[::1]:8001/v1",
            "https://127.8.9.10/v1",
            "http://user@127.0.0.1:1/x",
        ] {
            let mut cfg = baseline();
            main_model(&mut cfg).endpoint = endpoint.to_owned();
            assert!(issues(&cfg).is_empty(), "{endpoint}: {:#?}", issues(&cfg));
        }
    }

    #[test]
    fn v5_context_and_output_budget() {
        let mut cfg = baseline();
        main_model(&mut cfg).context = 0;
        assert_issue(&cfg, "V5", Severity::Error, "models.main.context");

        let mut cfg = baseline();
        main_model(&mut cfg).max_output_tokens = 16_384;
        assert_issue(&cfg, "V5", Severity::Error, "models.main.max_output_tokens");
    }

    #[test]
    fn v6_launch_runtime_must_exist() {
        let mut cfg = baseline();
        main_model(&mut cfg)
            .launch
            .as_mut()
            .expect("launch")
            .runtime = "missing".to_owned();
        assert_issue(&cfg, "V6", Severity::Error, "models.main.launch.runtime");
    }

    #[test]
    fn v22_extra_args_may_not_rebind_or_redirect_the_server() {
        for args in [
            vec!["--host", "0.0.0.0"],
            vec!["--port=9000"],
            vec!["-m", "/tmp/other.gguf"],
            vec!["--log-file", "C:\\Users\\x\\.bashrc"],
            vec!["--slot-save-path=/tmp"],
            vec!["-hf", "someone/model"],
        ] {
            let mut cfg = baseline();
            main_model(&mut cfg)
                .launch
                .as_mut()
                .expect("launch")
                .extra_args = args.iter().map(|a| (*a).to_owned()).collect();
            assert_issue(
                &cfg,
                "V22",
                Severity::Error,
                "models.main.launch.extra_args",
            );
        }
        let mut cfg = baseline();
        main_model(&mut cfg)
            .launch
            .as_mut()
            .expect("launch")
            .extra_args = vec!["--spec-type".to_owned(), "draft-mtp".to_owned()];
        assert_no_rule(&cfg, "V22");
    }

    #[test]
    fn v23_request_limits_are_bounded() {
        let mut cfg = baseline();
        main_model(&mut cfg).timeout_ms = 999;
        assert_issue(&cfg, "V23", Severity::Error, "models.main.timeout_ms");
        main_model(&mut cfg).timeout_ms = 3_600_001;
        assert_issue(&cfg, "V23", Severity::Error, "models.main.timeout_ms");

        let mut cfg = baseline();
        main_model(&mut cfg).retries = 11;
        assert_issue(&cfg, "V23", Severity::Error, "models.main.retries");

        let mut cfg = baseline();
        main_model(&mut cfg).timeout_ms = 3_600_000;
        main_model(&mut cfg).retries = 10;
        assert_no_rule(&cfg, "V23");
    }

    #[test]
    fn v7_prism_bonsai_needs_prism_runtime() {
        let mut cfg = baseline();
        main_model(&mut cfg).provider = ProviderKind::PrismBonsai;
        assert_issue(&cfg, "V7", Severity::Error, "models.main.launch.runtime");
    }

    #[test]
    fn v7_ternary_weights_need_prism_runtime() {
        let mut cfg = baseline();
        main_model(&mut cfg)
            .source
            .as_mut()
            .expect("source")
            .quantization = "PTQ1_0".to_owned();
        assert_issue(&cfg, "V7", Severity::Error, "models.main.launch.runtime");

        let mut cfg = baseline();
        main_model(&mut cfg)
            .launch
            .as_mut()
            .expect("launch")
            .model_file = PathBuf::from("models/Bonsai-8B-pq2_0.gguf");
        assert_issue(&cfg, "V7", Severity::Error, "models.main.launch.runtime");
    }

    #[test]
    fn v7_prism_runtime_accepts_ternary_bonsai() {
        let mut cfg = baseline();
        cfg.runtimes.get_mut("llama").expect("runtime").kind = RuntimeKind::PrismLlamaCpp;
        let model = main_model(&mut cfg);
        model.provider = ProviderKind::PrismBonsai;
        model.source.as_mut().expect("source").quantization = "PQ2_0".to_owned();
        assert_no_rule(&cfg, "V7");
    }

    #[test]
    fn v8_api_key_env_must_be_a_variable_name() {
        let mut cfg = baseline();
        main_model(&mut cfg).api_key_env = Some("sk-live-123".to_owned());
        assert_issue(&cfg, "V8", Severity::Error, "models.main.api_key_env");
        let message = issues(&cfg)
            .into_iter()
            .find(|i| i.rule == "V8")
            .map(|i| i.message)
            .unwrap_or_default();
        assert!(!message.contains("sk-live-123"), "secret echoed: {message}");

        let mut cfg = baseline();
        cfg.external.jev.api_key_env = Some("jev key".to_owned());
        assert_issue(&cfg, "V8", Severity::Error, "external.jev.api_key_env");

        let mut cfg = baseline();
        main_model(&mut cfg).api_key_env = Some("OPENAI_API_KEY".to_owned());
        assert_no_rule(&cfg, "V8");
    }

    #[test]
    fn v9_target_tokens_within_hard_limit() {
        let mut cfg = baseline();
        cfg.context.target_tokens = 30_000;
        assert_issue(&cfg, "V9", Severity::Error, "context.target_tokens");
    }

    #[test]
    fn v10_escalation_threshold_in_unit_range() {
        for threshold in [1.5_f32, -0.1, f32::NAN] {
            let mut cfg = baseline();
            cfg.routing.escalation_threshold = threshold;
            assert_issue(&cfg, "V10", Severity::Error, "routing.escalation_threshold");
        }
    }

    #[test]
    fn v11_jev_decisions_need_jev_enabled() {
        let mut cfg = baseline();
        cfg.decisions.provider = DecisionProviderKind::Jev;
        assert_issue(&cfg, "V11", Severity::Error, "decisions.provider");

        let mut cfg = baseline();
        cfg.decisions.fallback = DecisionProviderKind::Jev;
        assert_issue(&cfg, "V11", Severity::Error, "decisions.fallback");

        let mut cfg = baseline();
        cfg.decisions.provider = DecisionProviderKind::Jev;
        cfg.external.jev.enabled = true;
        assert_no_rule(&cfg, "V11");
    }

    #[test]
    fn v12_local_only_forbids_external_opt_ins() {
        let mut cfg = baseline();
        cfg.external.jev.enabled = true;
        assert_issue(&cfg, "V12", Severity::Error, "external.jev.enabled");

        let mut cfg = baseline();
        cfg.external.cloud_models.enabled = true;
        assert_issue(
            &cfg,
            "V12",
            Severity::Error,
            "external.cloud_models.enabled",
        );

        let mut cfg = baseline();
        cfg.profile = ProfileKind::Development;
        cfg.external.jev.enabled = true;
        cfg.external.cloud_models.enabled = true;
        assert_no_rule(&cfg, "V12");
    }

    #[test]
    fn v13_sampling_ranges() {
        let mut cfg = baseline();
        let sampling = &mut main_model(&mut cfg).sampling;
        sampling.temperature = Some(2.5);
        sampling.top_p = Some(0.0);
        sampling.min_p = Some(1.5);
        assert_issue(
            &cfg,
            "V13",
            Severity::Error,
            "models.main.sampling.temperature",
        );
        assert_issue(&cfg, "V13", Severity::Error, "models.main.sampling.top_p");
        assert_issue(&cfg, "V13", Severity::Error, "models.main.sampling.min_p");

        let mut cfg = baseline();
        let sampling = &mut main_model(&mut cfg).sampling;
        sampling.temperature = Some(0.7);
        sampling.top_p = Some(1.0);
        sampling.min_p = Some(0.0);
        assert_no_rule(&cfg, "V13");
    }

    #[test]
    fn v14_unknown_kv_cache_is_a_warning() {
        let mut cfg = baseline();
        main_model(&mut cfg).kv_cache = Some("q2_k".to_owned());
        assert_issue(&cfg, "V14", Severity::Warning, "models.main.kv_cache");
    }

    #[test]
    fn v15_required_verification_needs_steps() {
        let mut cfg = baseline();
        cfg.verification.steps.clear();
        assert_issue(&cfg, "V15", Severity::Warning, "verification.steps");
    }

    #[test]
    fn v16_relative_paths_need_their_base_directory() {
        let mut cfg = baseline();
        cfg.paths.models_dir = None;
        assert_issue(
            &cfg,
            "V16",
            Severity::Error,
            "models.main.launch.model_file",
        );

        let mut cfg = baseline();
        cfg.paths.runtimes_dir = None;
        assert_issue(&cfg, "V16", Severity::Error, "runtimes.llama.server_bin");

        let mut cfg = baseline();
        cfg.paths.models_dir = None;
        main_model(&mut cfg)
            .launch
            .as_mut()
            .expect("launch")
            .model_file = PathBuf::from(r"D:\models\x.gguf");
        assert_no_rule(&cfg, "V16");
    }

    #[test]
    fn v17_slots_sharing_an_endpoint_warn() {
        let mut cfg = baseline();
        let twin = cfg.models.get("main").expect("main slot").clone();
        cfg.models.insert("micro".to_owned(), twin);
        assert_issue(&cfg, "V17", Severity::Warning, "models.micro.endpoint");
    }

    #[test]
    fn v18_models_dir_on_system_drive_warns() {
        let mut cfg = baseline();
        cfg.paths.models_dir = Some(PathBuf::from(r"c:\models"));
        let env = ValidationEnv {
            system_drive: Some("C:".to_owned()),
        };
        assert!(
            validate_with(&cfg, &env)
                .iter()
                .any(|i| i.rule == "V18" && i.severity == Severity::Warning),
            "expected V18"
        );

        cfg.paths.models_dir = Some(PathBuf::from(r"D:\models"));
        assert!(
            validate_with(&cfg, &env).iter().all(|i| i.rule != "V18"),
            "V18 should not fire off the system drive"
        );
    }

    #[test]
    fn v19_fallback_should_be_deterministic() {
        let mut cfg = baseline();
        cfg.decisions.fallback = DecisionProviderKind::LocalLfm;
        assert_issue(&cfg, "V19", Severity::Warning, "decisions.fallback");
    }

    #[test]
    fn v20_mock_provider_only_in_test_profile() {
        let mut cfg = baseline();
        main_model(&mut cfg).provider = ProviderKind::Mock;
        assert_issue(&cfg, "V20", Severity::Error, "models.main.provider");

        cfg.profile = ProfileKind::Test;
        assert_no_rule(&cfg, "V20");
    }

    #[test]
    fn v21_hard_limit_within_default_model_context() {
        let mut cfg = baseline();
        assert_no_rule(&cfg, "V21");

        cfg.context.hard_limit = 16_385;
        assert_issue(&cfg, "V21", Severity::Error, "context.hard_limit");

        // A missing default model is V2's problem, not V21's.
        cfg.agent.default_model = "deep".to_owned();
        assert_issue(&cfg, "V2", Severity::Error, "agent.default_model");
        assert_no_rule(&cfg, "V21");
    }

    #[test]
    fn profile_spellings_parse_and_unknown_ones_fail() {
        for (text, kind) in [
            ("local-only", ProfileKind::LocalOnly),
            ("development", ProfileKind::Development),
            ("local-with-jev", ProfileKind::LocalWithJev),
            ("hybrid", ProfileKind::Hybrid),
            ("benchmark", ProfileKind::Benchmark),
            ("test", ProfileKind::Test),
        ] {
            let cfg = from_toml_str(&format!("profile = \"{text}\"\n")).expect("parses");
            assert_eq!(cfg.profile, kind);
            assert_eq!(kind.as_str(), text);
        }
        assert!(from_toml_str("profile = \"mock\"\n").is_err());
        assert!(from_toml_str("profile = \"LocalOnly\"\n").is_err());
    }

    #[test]
    fn shipped_profiles_validate_without_errors() {
        for (name, text) in [
            (
                "local-only",
                include_str!("../../../../configs/local-only.toml"),
            ),
            (
                "development",
                include_str!("../../../../configs/development.toml"),
            ),
        ] {
            let mut cfg = from_toml_str(text).unwrap_or_else(|err| panic!("{name}: {err:?}"));
            // The shipped files leave machine locations to the machine config (see their headers).
            cfg.paths.models_dir = Some(PathBuf::from("/data/models"));
            cfg.paths.runtimes_dir = Some(PathBuf::from("/data/runtimes"));
            let errors: Vec<_> = issues(&cfg)
                .into_iter()
                .filter(|i| i.severity == Severity::Error)
                .collect();
            assert!(errors.is_empty(), "{name}: {errors:#?}");
        }
    }

    #[test]
    fn endpoint_host_strips_scheme_userinfo_port_and_path() {
        assert_eq!(endpoint_host("http://127.0.0.1:8001/v1"), Some("127.0.0.1"));
        assert_eq!(
            endpoint_host("https://user@localhost?x=1"),
            Some("localhost")
        );
        assert_eq!(endpoint_host("HTTP://[::1]:8080/v1"), Some("[::1]"));
        assert_eq!(endpoint_host("http://"), Some(""));
        assert_eq!(endpoint_host("127.0.0.1:8001"), None);
        assert_eq!(endpoint_host("ftp://127.0.0.1"), None);
        assert_eq!(endpoint_host("http://[::1/v1"), None);
    }

    #[test]
    fn loopback_classification() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.255.0.1"));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.1"));
        assert!(!is_loopback_host("[::2]"));
        assert!(!is_loopback_host("example.com"));
    }
}
